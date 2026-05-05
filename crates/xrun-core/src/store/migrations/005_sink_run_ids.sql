-- v0.7: pluggable metric sinks. The runs table already carries
-- `mlflow_run_id` (added in 001); WandB needs the same pair (id + url) so
-- `xrun show` and the TUI can build clickable run links without round-
-- tripping wandb. Comet will land in v0.8 with the same shape.
--
-- The MLflow url is also captured here for symmetry — previously the TUI
-- rebuilt it client-side from `mlflow.url + experiment + run_id`, which
-- broke when the user later edited their MLflow URL. Storing the resolved
-- URL makes link rendering robust to config changes mid-run.

ALTER TABLE runs ADD COLUMN mlflow_run_url TEXT;
ALTER TABLE runs ADD COLUMN wandb_run_id TEXT;
ALTER TABLE runs ADD COLUMN wandb_run_url TEXT;
UPDATE schema_version SET version = 5;
