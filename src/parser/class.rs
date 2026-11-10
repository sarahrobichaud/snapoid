//! Minimal JVM class-file parser.
//!
//! Just enough to read a constant pool and scan bytecode for
//! `new X; dup; <push args>; invokespecial X.<init>` call sites. Used to
//! extract the game version constants baked into Project Zomboid's jar.

use anyhow::{bail, ensure, Context, Result};

#[derive(Debug)]
enum CpEntry {
    Utf8(String),
    Class { name_index: u16 },
    String { string_index: u16 },
    Member { class_index: u16, name_and_type_index: u16 },
    NameAndType { name_index: u16, descriptor_index: u16 },
    Other,
}

/// Parsed constant pool; indexes are 1-based per the JVM spec.
pub struct ConstantPool {
    entries: Vec<Option<CpEntry>>,
}

impl ConstantPool {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 10, "class file too short");
        ensure!(
            bytes[0..4] == [0xCA, 0xFE, 0xBA, 0xBE],
            "not a class file (bad magic)"
        );
        let count = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
        let mut entries: Vec<Option<CpEntry>> = (0..count.max(1)).map(|_| None).collect();
        let mut pos = 10;
        let mut index = 1usize;
        while index < count {
            let tag = *bytes
                .get(pos)
                .with_context(|| format!("constant pool entry {index} past end of file"))?;
            pos += 1;
            let entry = match tag {
                1 => {
                    let len = read_u16(bytes, pos)?;
                    pos += 2;
                    let text =
                        String::from_utf8_lossy(slice(bytes, pos, len as usize)?).into_owned();
                    pos += len as usize;
                    CpEntry::Utf8(text)
                }
                3 | 4 => {
                    pos += 4;
                    CpEntry::Other
                }
                5 | 6 => {
                    pos += 8;
                    CpEntry::Other
                }
                7 => {
                    let name_index = read_u16(bytes, pos)?;
                    pos += 2;
                    CpEntry::Class { name_index }
                }
                8 => {
                    let string_index = read_u16(bytes, pos)?;
                    pos += 2;
                    CpEntry::String { string_index }
                }
                9 | 10 | 11 => {
                    let class_index = read_u16(bytes, pos)?;
                    let name_and_type_index = read_u16(bytes, pos + 2)?;
                    pos += 4;
                    CpEntry::Member { class_index, name_and_type_index }
                }
                12 => {
                    let name_index = read_u16(bytes, pos)?;
                    let descriptor_index = read_u16(bytes, pos + 2)?;
                    pos += 4;
                    CpEntry::NameAndType { name_index, descriptor_index }
                }
                15 => {
                    pos += 3;
                    CpEntry::Other
                }
                16 => {
                    pos += 2;
                    CpEntry::Other
                }
                17 | 18 => {
                    pos += 4;
                    CpEntry::Other
                }
                19 | 20 => {
                    pos += 2;
                    CpEntry::Other
                }
                other => bail!("unexpected constant pool tag {other} at entry {index}"),
            };
            entries[index] = Some(entry);
            index += 1;
            if tag == 5 || tag == 6 {
                // long/double occupy two pool slots
                index += 1;
            }
        }
        Ok(Self { entries })
    }

    fn entry(&self, index: u16) -> Option<&CpEntry> {
        self.entries.get(index as usize).and_then(|e| e.as_ref())
    }

    pub fn utf8(&self, index: u16) -> Option<&str> {
        match self.entry(index) {
            Some(CpEntry::Utf8(text)) => Some(text),
            _ => None,
        }
    }

    pub fn class_name(&self, index: u16) -> Option<String> {
        match self.entry(index) {
            Some(CpEntry::Class { name_index }) => self.utf8(*name_index).map(str::to_owned),
            _ => None,
        }
    }

    pub fn string(&self, index: u16) -> Option<String> {
        match self.entry(index) {
            Some(CpEntry::String { string_index }) => self.utf8(*string_index).map(str::to_owned),
            _ => None,
        }
    }

    /// (class name, member name, descriptor) for a field/method ref entry.
    pub fn member(&self, index: u16) -> Option<(String, String, String)> {
        let (class_index, name_and_type_index) = match self.entry(index) {
            Some(CpEntry::Member { class_index, name_and_type_index }) => {
                (*class_index, *name_and_type_index)
            }
            _ => return None,
        };
        let (name_index, descriptor_index) = match self.entry(name_and_type_index) {
            Some(CpEntry::NameAndType { name_index, descriptor_index }) => {
                (*name_index, *descriptor_index)
            }
            _ => return None,
        };
        Some((
            self.class_name(class_index)?,
            self.utf8(name_index)?.to_owned(),
            self.utf8(descriptor_index)?.to_owned(),
        ))
    }

    /// Number of pool slots (including the unused index 0).
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A parsed `new X(...)` call site.
#[derive(Debug)]
pub struct ConstructorCall {
    pub int_args: Vec<i32>,
    pub string_arg: Option<String>,
    /// Offset just past the `invokespecial`, i.e. where the next
    /// instruction (e.g. putstatic/areturn) begins.
    pub end: usize,
}

/// Find `new <class>; dup; <push args>; invokespecial <class>.<method>`
/// sites in raw bytecode. Only the simple push instructions emitted for
/// constant arguments are recognized (aconst_null, iconst, bipush, sipush,
/// ldc, ldc_w); anything else aborts that call site.
pub fn find_constructor_calls(
    code: &[u8],
    pool: &ConstantPool,
    class_name: &str,
    method_name: &str,
) -> Vec<ConstructorCall> {
    let mut calls = Vec::new();
    let mut i = 0usize;
    while i + 4 < code.len() {
        if code[i] != 0xBB {
            // new
            i += 1;
            continue;
        }
        let Ok(new_index) = read_u16(code, i + 1) else {
            break;
        };
        if pool.class_name(new_index).as_deref() != Some(class_name) {
            i += 1;
            continue;
        }
        if code[i + 3] != 0x59 {
            // dup
            i += 1;
            continue;
        }

        let mut j = i + 4;
        let mut int_args = Vec::new();
        let mut string_arg = None;
        let mut matched = false;
        while j + 2 < code.len() {
            match code[j] {
                0x10 => {
                    // bipush (signed byte operand)
                    int_args.push(code[j + 1] as i8 as i32);
                    j += 2;
                }
                0x11 => {
                    // sipush (signed short operand)
                    int_args.push(i16::from_be_bytes([code[j + 1], code[j + 2]]) as i32);
                    j += 3;
                }
                0x01 => {
                    // aconst_null
                    j += 1;
                }
                0x02 => {
                    // iconst_m1
                    int_args.push(-1);
                    j += 1;
                }
                0x03..=0x08 => {
                    // iconst_0..iconst_5
                    int_args.push((code[j] - 0x03) as i32);
                    j += 1;
                }
                0x12 => {
                    // ldc
                    if string_arg.is_none() {
                        string_arg = pool.string(code[j + 1] as u16);
                    }
                    j += 2;
                }
                0x13 => {
                    // ldc_w
                    if string_arg.is_none() {
                        if let Ok(index) = read_u16(code, j + 1) {
                            string_arg = pool.string(index);
                        }
                    }
                    j += 3;
                }
                0xB7 => {
                    // invokespecial
                    if let Ok(index) = read_u16(code, j + 1) {
                        if let Some((cls, name, _)) = pool.member(index) {
                            matched = cls == class_name && name == method_name;
                        }
                    }
                    j += 3;
                    break;
                }
                _ => break,
            }
        }
        if matched {
            calls.push(ConstructorCall { int_args, string_arg, end: j });
        }
        i = j.max(i + 1);
    }
    calls
}

fn read_u16(bytes: &[u8], pos: usize) -> Result<u16> {
    let chunk = slice(bytes, pos, 2)?;
    Ok(u16::from_be_bytes([chunk[0], chunk[1]]))
}

fn slice(bytes: &[u8], pos: usize, len: usize) -> Result<&[u8]> {
    bytes
        .get(pos..pos + len)
        .with_context(|| format!("class file truncated at byte {pos}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal class file whose pool contains a MethodRef to
    /// `zombie/core/GameVersion.<init>(IILjava/lang/String;)V` and a
    /// String constant "", followed by raw `code` bytes.
    fn fixture_class(code: &[u8]) -> Vec<u8> {
        fn utf8(text: &str) -> Vec<u8> {
            let mut entry = vec![1u8];
            entry.extend((text.len() as u16).to_be_bytes());
            entry.extend(text.as_bytes());
            entry
        }

        let mut bytes = Vec::new();
        bytes.extend([0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend([0x00, 0x00, 0x00, 0x00]); // minor, major
        bytes.extend([0x00, 0x09]); // pool count: entries 1..8
        bytes.extend(utf8("zombie/core/GameVersion")); // 1
        bytes.extend([7, 0, 1]); // 2: Class -> 1
        bytes.extend(utf8("<init>")); // 3
        bytes.extend(utf8("(IILjava/lang/String;)V")); // 4
        bytes.extend([12, 0, 3, 0, 4]); // 5: NameAndType
        bytes.extend([10, 0, 2, 0, 5]); // 6: MethodRef
        bytes.extend(utf8("")); // 7: ""
        bytes.extend([8, 0, 7]); // 8: String -> 7
        bytes.extend(code);
        bytes
    }

    #[test]
    fn finds_constructor_calls() {
        // new GameVersion; dup; bipush 42; bipush 22; ldc_w ""; invokespecial; putstatic
        let code = [0xBB, 0, 2, 0x59, 0x10, 0x2A, 0x10, 0x16, 0x13, 0, 8, 0xB7, 0, 6, 0xB3];
        let bytes = fixture_class(&code);
        let pool = ConstantPool::parse(&bytes).unwrap();
        let calls = find_constructor_calls(&bytes, &pool, "zombie/core/GameVersion", "<init>");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].int_args, vec![42, 22]);
        assert_eq!(calls[0].string_arg.as_deref(), Some(""));
        assert_eq!(calls[0].end, bytes.len() - 1); // points at the putstatic
    }

    #[test]
    fn handles_iconst_and_filters_other_classes() {
        // new GameVersion; dup; bipush 42; iconst_0; ldc_w ""; invokespecial; areturn
        let code = [0xBB, 0, 2, 0x59, 0x10, 0x2A, 0x03, 0x13, 0, 8, 0xB7, 0, 6, 0xB0];
        let bytes = fixture_class(&code);
        let pool = ConstantPool::parse(&bytes).unwrap();
        let calls = find_constructor_calls(&bytes, &pool, "zombie/core/GameVersion", "<init>");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].int_args, vec![42, 0]);
        assert_eq!(calls[0].string_arg.as_deref(), Some(""));

        let none = find_constructor_calls(&bytes, &pool, "zombie/core/Other", "<init>");
        assert!(none.is_empty());
    }

    #[test]
    fn rejects_non_class_bytes() {
        assert!(ConstantPool::parse(b"not a class file").is_err());
    }
}
