# [XS] CI runs once per pull request

## Goal

A push to a pull request branch runs each job once, not once for `push` and once for
`pull_request`.

## Plan

Limit `push` to `main`, and skip draft PRs: add `ready_for_review` to the `pull_request` types
and give each job `if: github.event_name != 'pull_request' || github.event.pull_request.draft == false`.

Decided 2026-11-18 (maintainer): ✅ skip CI on drafts, as Quest's setup recommends, since every quest
claim is a draft holding an empty commit (rejected: run CI on drafts too).
