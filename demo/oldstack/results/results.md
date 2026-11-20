Measured 2026-11-19 on one machine (Intel(R) Xeon(R) Processor, 8 cores, Python 3.13.5, mosquitto 2.0.21). Software path only: the radio is replaced by localhost UDP. Airtime columns are computed, not measured.

### Paced runs (payloads sent on their schedule)

| workload | air loss | stack | delivered | decodable (GOP rule) | latency p50 / p95 / p99 ms | air bytes per payload | backhaul bytes per payload | CPU ms per payload |
|---|---|---|---|---|---|---|---|---|
| sensor | 0% | old stack (Python) | 100.0% | n/a | 1.07 / 1.68 / 2.44 | 25 | 651 | 1.86 |
| sensor | 0% | moq-lora (Rust) | 100.0% | n/a | 0.16 / 0.23 / 0.27 | 192 | 0 | 0.10 |
| sensor | 1% | old stack (Python) | 99.0% | n/a | 1.19 / 1.61 / 2.24 | 25 | 645 | 1.93 |
| sensor | 1% | moq-lora (Rust) | 100.0% | n/a | 0.16 / 0.21 / 0.25 | 192 | 0 | 0.10 |
| video | 0% | old stack (Python) | 100.0% | 100.0% | 43.72 / 49.87 / 89.06 | 1621 | 8310 | 4.83 |
| video | 0% | moq-lora (Rust) | 100.0% | 100.0% | 0.19 / 0.27 / 0.36 | 1892 | 0 | 0.15 |
| video | 1% | old stack (Python) | 94.3% | 69.0% | 43.17 / 49.69 / 90.96 | 1621 | 8234 | 4.79 |
| video | 1% | moq-lora (Rust) | 99.7% | 96.0% | 0.18 / 0.29 / 0.36 | 1892 | 0 | 0.16 |

### Unpaced runs (as fast as the software goes, no loss)

| workload | stack | delivered | payloads/s | goodput kbit/s | latency p50 / p99 ms |
|---|---|---|---|---|---|
| sensor | old stack (Python) | 85.0% | 4222 | 405 | 22.8 / 74.8 |
| sensor | moq-lora (Rust) | 100.0% | 174972 | 16797 | 0.0 / 0.1 |
| video | old stack (Python) | 62.7% | 805 | 9685 | 51.2 / 61.3 |
| video | moq-lora (Rust) | 100.0% | 60521 | 728501 | 0.0 / 0.2 |

### Time on air (model, lossless)

| workload | stack | radio | frames | airtime per payload ms | channel busy at the offered rate | time to send it all |
|---|---|---|---|---|---|---|
| sensor | old stack | LoRa SF8/500 kHz CR4/5, 8-symbol preamble (US915 DR4) | 400 | 28.29 | 57% | 811 s for 20 s of traffic (Class A receive windows) |
| sensor | moq-lora | FSK 300 kbit/s, 6-byte preamble, 2-byte sync, CRC-16 | 800 | 5.71 | 11% | 20 s (keeps up) |
| video | old stack | LoRa SF8/500 kHz CR4/5, 8-symbol preamble (US915 DR4) | 2045 | 1128.58 | 1693% | 4429 s for 20 s of traffic (Class A receive windows) |
| video | moq-lora | FSK 300 kbit/s, 6-byte preamble, 2-byte sync, CRC-16 | 2395 | 52.80 | 79% | 20 s (keeps up) |

### Command round trip (app to device and back, 10 and 10 commands)

| stack | p50 ms | p95 ms | max ms | timeouts | airtime to add on a real radio ms |
|---|---|---|---|---|---|
| old stack, Class A, uplink every 2 s | 1936.6 | 2699.5 | 2699.5 | 0 | 37.3 |
| moq-lora | 0.27 | 152.87 | 152.87 | 0 | 4.1 |
