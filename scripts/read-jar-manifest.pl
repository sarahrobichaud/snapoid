#!/usr/bin/env perl
# One-off: print a jar entry to stdout (used to inspect PZ's jar manifest).
use IO::Uncompress::Unzip qw(unzip);

my $jar = shift or die "usage: read-jar-manifest.pl <jar> [entry]\n";
my $entry = shift // "META-INF/MANIFEST.MF";

unzip $jar => \my $out, Name => $entry
    or die "unzip failed: $IO::Uncompress::Unzip::UnzipError\n";
print $out;
