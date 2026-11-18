# [L] Encryption covers the radio frame headers and parity

## Goal

With `--key-file`, an on-path attacker cannot drop, reorder, or retarget fragments undetected:
sequence, fragment index, `nfrag`, the parity bit, `k`, and `objlen` are authenticated.

## Plan

Today only `group` and `obj` are AAD (`moq-lora-wire/src/seal.rs`). This is a wire change: bump the
frame `VERSION` and say so in the PR. Weigh a per-frame tag (airtime on every frame) against
authenticating the header fields an object's frames share inside the object tag. Invalidates the
encrypted runs. It also closes the one gap left in loss accounting: two consecutive forged frames
far ahead are a confirmed gap, since sequence numbers are unauthenticated (`SEQ_STEP` in
`moq-lora-wire/src/lib.rs`).
