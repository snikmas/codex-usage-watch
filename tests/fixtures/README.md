# Stage 0 Fixture Oracle

These JSONL files contain rate-limit metadata only. Their envelope and rate-limit shapes were taken from local Codex transcripts, then timestamps, paths, identifiers, and selected percentages were normalized. They contain no prompt or response content.

Unless noted otherwise, `now` is the time of the final valid observation, the local window duration is five hours, the stale threshold is 15 minutes, and calibration is `15.8` weekly points.

| Fixture | Hand calculation | Expected display/state |
|---|---|---|
| `real_weekly_only.jsonl` | Weekly window is `primary`; `5 -> 5`, delta `0` | `5h est 0% · week +0.0%`; fresh |
| `real_dual_window.jsonl` | Weekly window is `secondary`; `56 -> 57`, delta `1`; the unrelated 300-minute reset is ignored | `5h est 6% · week +1.0%`; fresh |
| `normal_growth.jsonl` | `20 -> 22`, delta `2`; `2 / 15.8 * 100 = 12.658227...` | `5h est 13% · week +2.0%`; fresh |
| `no_growth.jsonl` | `20 -> 20 -> 20`, delta `0` | `5h est 0% · week +0.0%`; fresh |
| `expiry.jsonl` | First window gains `22 - 20 = 2`; event exactly at `17:00` is outside `[12:00, 17:00)` and starts a new baseline at `25` | archived window `13% / +2.0%`; current window `0% / +0.0%`, starts `17:00` |
| `weekly_reset.jsonl` | `98 -> 99` adds `1`; reset to `1` adds post-reset `1`; `1 -> 3` adds `2`; total `4`; `4 / 15.8 * 100 = 25.316455...` | `5h est 25% · week +4.0%`; fresh |
| `duplicate_events.jsonl` | `20 -> 21` adds `1`; repeated source/offset is ignored | `5h est 6% · week +1.0%`; fresh |
| `malformed_final_line.jsonl` | `20 -> 21` adds `1`; incomplete last line adds nothing and is retryable | `5h est 6% · week +1.0%`; fresh; refresh succeeds with warning |
| `concurrent_a.jsonl` + `concurrent_b.jsonl` | Merge by time: `20 -> 21 -> 21 -> 22`; deltas `1 + 0 + 1 = 2` | `5h est 13% · week +2.0%`; one shared window |

Additional policy checks derived from the same cases:

- At exactly 15 minutes after the newest accepted observation, the reading is fresh; one instant later it is stale.
- Re-evaluating any fixture with no new records produces the identical state.
- Above 100% is not clamped: a delta of `18` displays `114%` and remains fail-open.
- If every line is malformed or lacks a 10080-minute window, status is `unknown`, not zero.

`duplicate_events.jsonl` includes test-only `_fixture_source` and `_fixture_offset` fields to make the repeated `ObservationId` explicit. Production ingestion derives those values from the actual file and byte offset rather than trusting JSON fields.
