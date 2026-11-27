# VIOFO parking GPS

The validated VIOFO filename suffix `PF`, `PI` or `PR` selects parking handling,
including firmware names without a sequence counter. `P` does not identify the
parking submode, capture rate or an exact GPS-to-video time mapping.

Parking GPS observations keep the existing clock-order and spatial consistency
checks, without bounding GPS wall time by playback duration. A time-lapse file
can legitimately contain many minutes of GPS observations in a short video.

Until its media-time association is established, a parking file contributes one
representative location: an actual validated fix nearest the coordinate-wise
median. This is a file-level location anchor at offset zero, not a reconstructed
moving GPS track or proof the vehicle never moved. Instantaneous speed/heading
are unavailable and use the shared UI's neutral zero convention. No capture
factor, civil-timezone alignment or nearest-time channel pairing is inferred.
Files without usable GPS remain playable without a location.

Only affected parking source signatures change, so existing parking analysis
is revisited when originals are available. Ordinary driving signatures and the
main archive schema remain unchanged; PR #21 still uses schema 13. Existing
protected-recording and trip-boundary policy is not changed by this patch.

The A129 temporal-exclusion report from @alinnastac and the owner's separate
A229 Pro time-lapse/low-bitrate diagnostics motivated the regression cases.
Tests use synthetic coordinates and clocks, not contributor footage. They
reproduce the 110/18 duration-bound exclusions and cover legacy/current P
suffixes. These tests are not hardware acceptance of this new revision.

The independently reported filename differences of up to five seconds and
camera-pair alignment remain open. This GPS correction does not claim to fix
them. See [isolated self-build instructions](VIOFO_TESTING.md); test one existing
parking clip if convenient. No new recording or footage upload is needed.
