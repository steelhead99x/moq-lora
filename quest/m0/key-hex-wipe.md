# [XS] Every copy of the hex key is wiped

## Goal

Loading a key file leaves no unwiped copy of the key in freed memory: `parse_key_hex` wipes its
whitespace-stripped string, and neither it nor `decode_hex` reallocates mid-build.

## Plan

`REVIEW.md` says the hex is wiped; `parse_key_hex` made a second copy that was not.
