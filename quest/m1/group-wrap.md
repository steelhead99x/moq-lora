# [S] Object 255 never opens a group without a keyframe

## Goal

When a GOP exceeds 255 objects, the receiver is never handed a non-IDR as a random access point.

## Plan

Today the sender opens a new group at object 255 even without an IDR. Either request an IDR from
the encoder or mark the group as not decodable so the gate skips it. GOP 15 never hits this.
