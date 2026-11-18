# [XS] Key file is checked and read through one handle

## Goal

The key file's mode is checked on the open file (`fstat`), so it cannot change between the check
and the read.
