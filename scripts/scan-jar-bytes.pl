#!/usr/bin/env perl
# One-off: list jar entries whose content contains a byte pattern given as
# colon-separated hex, e.g. 10:2A:10:16
use IO::Uncompress::Unzip;

my $jar = shift or die "usage: scan-jar-bytes.pl <jar> <hex:hex:...>\n";
my $pat = pack "C*", map { hex $_ } split /:/, shift;

my $z = IO::Uncompress::Unzip->new($jar)
    or die "open failed: $IO::Uncompress::Unzip::UnzipError\n";

my ($status, $buff);
for ($status = 1; $status > 0; $status = $z->nextStream()) {
    my $name = $z->getHeaderInfo()->{Name};
    my $data = '';
    while (($status = $z->read($buff)) > 0) { $data .= $buff; }
    print "$name\n" if index($data, $pat) >= 0;
}
