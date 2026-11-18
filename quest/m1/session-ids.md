# [S] Two runs under one key never share a session id

## Goal

Nonces stay unique for a key's whole life, not just within one run: no two sender runs under the
same key can draw the same session id, so no ChaCha20 or AES-GCM keystream is ever reused.

## Plan

Today each run draws a random 32-bit session (`moq-lora-send`, `random_u32()`), and the nonce is
`session | group | obj` (`moq-lora-wire/src/seal.rs`). Two runs collide with probability about
n^2 / 2^33 after n runs, which `REVIEW.md` defers as the birthday bound and the README tells users
to manage by rotating the key. Options: persist a per-key counter next to the key file, or derive
a per-run subkey from the key and a random 96-bit salt sent in the clear, which removes the bound
without state. The salt is a wire change.
