# Query benchmark pipeline

This Benchmark plan divides ten problems into batches of five. Each batch gets one fresh Questioner
and one fresh Answerer child; the pair handles its problems sequentially without preflight or Session
forks. Query engine assets are installed from a Host-provided knowledge-factory bundle; the plan
contains neither generated A1-A4 outputs nor expected answers.

At start Labflow fills `problem/<id>/` in the plan workspace and triggers each batch once. The
Questioner reuses `ch/` across the batch and must write a nonempty `ch/out/report.md` for every
problem. The Answerer may write `ch/out/ok-*` or `ch/out/err-*` evidence; both families may be absent
and cannot coexist. Recording moves the stable result to `result/<id>/`, clears the channel, and
adds a per-problem row to `result/stats.jsonl`.
