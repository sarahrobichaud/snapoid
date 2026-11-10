#!/usr/bin/env perl
# One-off: dump specific constant-pool entries of a Java class file (stdin).
# Usage: dump-cp.pl <index> [index ...]
use strict;
use warnings;

my $class = do { local $/; <STDIN> };
my $pos = 8; # skip magic (4) + minor (2) + major (2)
my $count = unpack "n", substr($class, $pos, 2);
$pos += 2;

my @pool; # 1-based
my $i = 1;
while ($i < $count) {
    my $tag = unpack "C", substr($class, $pos, 1);
    $pos++;
    my $size = {1 => sub { 3 + unpack("n", substr($class, $pos, 2)) },
                3 => 5, 4 => 5, 5 => 9, 6 => 9, 7 => 3, 8 => 3,
                9 => 5, 10 => 5, 11 => 5, 12 => 5,
                15 => 4, 16 => 3, 17 => 5, 18 => 5, 19 => 3, 20 => 3}->{$tag}
        or die "unknown tag $tag at entry $i\n";
    $size = $size->() if ref $size;
    $pool[$i] = substr($class, $pos - 1, $size);
    $pos += $size - 1;
    $i++;
    $i++ if $tag == 5 || $tag == 6; # long/double take two slots
    if ($ENV{CP_DEBUG} && ($i % 100 == 0)) {
        printf STDERR "entry %d tag %d pos %d\n", $i, $tag, $pos;
    }
}

sub utf8 { my $e = $pool[shift]; return undef unless defined $e && ord(substr($e, 0, 1)) == 1;
    return substr($e, 3, unpack("n", substr($e, 1, 2))); }
sub name_of { # resolve Class(7)/String(8)/Methodref(10)/NameAndType(12) to text
    my $e = $pool[shift]; return "?" unless defined $e;
    my $tag = ord(substr($e, 0, 1)); my $idx = unpack("n", substr($e, 1, 2));
    if ($tag == 7) { return "Class " . (utf8($idx) // "?"); }
    if ($tag == 8) { return "String \"" . (utf8($idx) // "?") . "\""; }
    if ($tag == 10 || $tag == 11 || $tag == 9) {
        my $cls = unpack("n", substr($e, 1, 2)); my $nt = unpack("n", substr($e, 3, 2));
        my $nte = $pool[$nt]; my $name = utf8(unpack("n", substr($nte, 1, 2)));
        my $desc = utf8(unpack("n", substr($nte, 3, 2)));
        return "Member $cls.$name $desc";
    }
    return "tag $tag idx $idx";
}

printf "#%-5d %s\n", $_, name_of($_) for @ARGV;
