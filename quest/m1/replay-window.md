# [M] Receiver rejects replayed objects

## Goal

A recorded valid object replayed later, including an object 0, is rejected and counted, so the
group gate cannot be fed an old group.

## Plan

Track accepted `(session, group, obj)` in a bounded window. A late fragment that completes an
honest object must still pass, so the window keys on completed objects, not fragments.

Replayed frames also break reassembly today, key or no key: a frame from an older object
finalizes the object in progress. Measured while fixing loss accounting: 17 stale frames
replayed into a 30 s loopback run raised `objects_lost` from 15 to 60. The reassembler should
ignore frames for objects older than the one in progress.
