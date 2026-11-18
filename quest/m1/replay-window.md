# [M] Receiver rejects replayed objects

## Goal

A recorded valid object replayed later, including an object 0, is rejected and counted, so the
group gate cannot be fed an old group.

## Plan

Track accepted `(session, group, obj)` in a bounded window. A late fragment that completes an
honest object must still pass, so the window keys on completed objects, not fragments.
