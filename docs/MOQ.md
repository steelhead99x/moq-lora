# Republish to a MoQ relay (moq.dev, moq.pro)

The radio link carries video the MoQ way: one group per keyframe interval, one object per
frame. Once the laptop has it, `moq-lora-publish` sends the same groups and frames on as a real
Media over QUIC broadcast, built on [moq.dev](https://moq.dev)'s Rust crates. A moq.dev relay
then fans it out, and anyone can watch it in a browser.

```
Pi 2 ──▶ 915 MHz FSK ──▶ CatWAN stick ──▶ laptop: moq-lora-recv ──▶ moq-lora-publish ──▶ MoQ relay ──▶ viewers
```

## How it maps

| moq-lora (radio)                | MoQ broadcast                                   |
|---------------------------------|-------------------------------------------------|
| group `g` (one GOP)             | group `g` on the `video` track (counts on past 65535) |
| object `o` (one frame)          | frame `o` of that group, in order               |
| lost object                     | the MoQ group ends there; the next group starts clean |
| H.264 with SPS/PPS in band      | `avc3` in the `catalog.json` track (hang format) |
| capture time                    | frame timestamp, microseconds from the first frame |

Only objects that passed the receiver's group gate are sent, the same ones the decoder gets.
Encryption over the radio is unchanged: frames are decrypted on the laptop, and the relay
connection is protected by TLS (and a token on moq.pro).

## Build

`moq-lora-publish` is its own crate, outside the radio workspace, because moq.dev's crates need
Rust 1.91. The Pi binaries do not change.

```sh
cargo build --release                                       # moq-lora-send / moq-lora-recv
cargo build --release --manifest-path moq-lora-publish/Cargo.toml
export PATH="$PWD/moq-lora-publish/target/release:$PATH"   # so recv can find it
```

It uses `moq-net` 0.3, `moq-tokio` 0.19 (rustls with ring, no cmake) and `hang` 0.21.

## Local relay

Install `moq-relay` (`cargo install moq-relay`, or a binary from the
[moq-dev/moq releases](https://github.com/moq-dev/moq/releases)) and run it for development:

```sh
moq-relay --listen '[::]:4443' --listen-tls-generate localhost \
  --web-http-listen '[::]:4443' --auth-public 'anon/**'
```

Then add `--publish` to the receiver. With no radios, a loopback file works too:

```sh
./target/release/moq-lora-send --loopback /tmp/air.txt --secs 20
./target/release/moq-lora-recv --replay /tmp/air.txt --paced --out /tmp/run \
  --publish http://localhost:4443/anon --publish-name lora
```

`http://` makes the client fetch the relay's self-signed certificate fingerprint, which is fine
on localhost. Use `https://` for a relay with a real certificate.

## moq.pro

[moq.pro](https://moq.pro) is the hosted MoQ CDN from the people behind moq.dev, so there is no
relay to run. Make a project there, then use the relay URL and a publish token from the project
dashboard (the token goes in `?jwt=`):

```sh
moq-lora-recv --port /dev/cu.usbmodemXXXX ... \
  --publish 'https://cdn.moq.pro/...?jwt=YOUR_TOKEN' --publish-name lora
```

Keep the token out of shell history and scripts you share. `moq-lora-publish` also reads the
URL from `MOQ_LORA_RELAY`.

## Watch

moq.dev's web player is a web component. Point it at the same relay and broadcast name:

```html
<script type="module">import "https://esm.sh/@moq/watch/element";</script>
<moq-watch url="http://localhost:4443/anon" name="lora"><canvas></canvas></moq-watch>
```

## Tests

```sh
cd moq-lora-publish
cargo test                                   # unit tests + publish through an in-process relay
MOQ_LORA_TEST_RELAY=http://localhost:4443/anon cargo test   # the same against a running moq-relay
# The whole path from radio frames: recv --replay --publish, checked against rec.h264
MOQ_LORA_RECV=../target/release/moq-lora-recv MOQ_LORA_REPLAY=/tmp/air.txt \
  cargo test --test relay -- --ignored
```

## Limits

* One video track, no audio.
* The catalog's picture size comes from `--size`; it is not read from the SPS.
* The relay link uses the laptop's normal network. If it drops, `moq-lora-publish` reconnects;
  groups made while it is down are kept for up to 30 s.
* Not yet watched in a browser as part of the tests; the tests check every frame through a
  native subscriber.
