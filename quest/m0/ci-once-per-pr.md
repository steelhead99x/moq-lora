# [XS] CI runs once per pull request

## Goal

A push to a pull request branch runs each job once, not once for `push` and once for
`pull_request`.

## Plan

Limit `push` to `main`. Skipping draft PRs, as Quest's setup recommends, is a separate
decision for the maintainer, under Waiting.

## Waiting

- Maintainer: also skip CI on draft PRs, as Quest's setup recommends (add `ready_for_review` to the
  `pull_request` types and a `draft == false` condition per job)? Recommended: yes, since every
  quest claim is a draft holding an empty commit.
