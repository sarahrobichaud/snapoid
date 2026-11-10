#!/usr/bin/env perl
# One-off: search a class file (stdin) for byte patterns and print hex
# context around each hit. Args: hex patterns like 10:2A:10:16 or .2
use strict;
use warnings;

my $class = do { local $/; <STDIN> };

for my $spec (@ARGV) {
    my $pat = $spec =~ /^\./ ? $spec
        : pack("C*", map { hex $_ } split /:/, $spec);
    my $start = 0;
    my $count = 0;
    while (($start = index($class, $pat, $start)) >= 0) {
        my $from = $start > 24 ? $start - 24 : 0;
        my $ctx = substr($class, $from, 72);
        my $hex = join ' ', map { sprintf '%02x', $_ } unpack "C*", $ctx;
        print "$spec @ $start: $hex\n";
        $start += 1;
        last if ++$count >= 5;
    }
    print "$spec: no hits\n" if $count == 0;
}
