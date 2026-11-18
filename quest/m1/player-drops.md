# [XS] Access units dropped for a slow player are counted

## Goal

The receiver's summary counts access units its `try_send` discarded because ffplay or the decoder
fell behind.
