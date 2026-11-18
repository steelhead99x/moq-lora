# [XS] The CRC sealer and the AEAD module have different names

## Goal

`moq_lora_wire::seal` (the frame CRC function) is renamed so it no longer shares a name with the
`seal` (AEAD) module.
