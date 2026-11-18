# [M] Decode without the CLI's held frame

## Goal

The receiver decodes through libavcodec directly, so glass-to-glass latency loses the ~67 ms the
ffmpeg CLI holds back.
