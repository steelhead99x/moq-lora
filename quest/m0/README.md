# m0: shipped-tool bugs

## Goal

The send, receive, and report tools exit non-zero on every failure, keep their summary when a run
ends early, write valid JSON, and report the right direction and latency for every run.

## Waiting

- Maintainer: GitHub Actions refuses to start jobs ("recent account payments have failed or your
  spending limit needs to be increased"). Fix it under Settings, Billing and plans; done when a
  push's check runs start instead of failing in seconds.
