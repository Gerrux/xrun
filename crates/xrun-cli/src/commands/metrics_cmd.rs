#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{bail, Context, Result};
use xrun_core::{GlobalConfig, RunId, Store};

use crate::cli::MetricsArgs;

pub fn run(args: &MetricsArgs, db_path: &Path, config_dir: &Path) -> Result<()> {
    let id: RunId = args
        .id
        .parse()
        .with_context(|| format!("invalid run ID: {}", args.id))?;

    let store = Store::open(db_path)
        .with_context(|| format!("failed to open store at {}", db_path.display()))?;

    let run = store
        .get_run(&id)
        .context("failed to query run")?
        .ok_or_else(|| anyhow::anyhow!("run not found: {}", args.id))?;

    // --mlflow-url: print the URL to this run in MLflow UI
    if args.mlflow_url {
        let global = GlobalConfig::load(config_dir).unwrap_or_default();
        let base = global
            .mlflow
            .url
            .as_deref()
            .context("mlflow.url not set in config — run `xrun config set mlflow.url <url>`")?;
        let mlflow_run_id = run.mlflow_run_id.as_deref().context(
            "no MLflow run linked to this run (was it launched with mlflow.experiment set?)",
        )?;
        // MLflow UI URL format: <base>/#/experiments/<exp_id>/runs/<mlflow_run_id>
        // We don't store exp_id separately, so use the simpler runs-only URL.
        let url = format!(
            "{}/{}/#/runs/{}",
            base.trim_end_matches('/'),
            "",
            mlflow_run_id
        );
        println!("{url}");
        return Ok(());
    }

    // --png: render metrics to a PNG chart
    if let Some(ref png_path) = args.png {
        let filter_keys: Option<Vec<String>> = args
            .key
            .as_deref()
            .map(|s| s.split(',').map(str::trim).map(str::to_string).collect());

        let all_keys = store
            .list_metric_keys(&id)
            .context("failed to list metric keys")?;

        if all_keys.is_empty() {
            bail!("no metrics for run {} — nothing to plot", args.id);
        }

        let keys_to_plot: Vec<String> = match &filter_keys {
            Some(k) => k.clone(),
            None => all_keys.iter().map(|(k, _)| k.clone()).collect(),
        };

        render_png(&store, &id, &keys_to_plot, png_path)
            .with_context(|| format!("failed to render PNG to {}", png_path.display()))?;

        eprintln!("metrics chart saved to {}", png_path.display());
        return Ok(());
    }

    let filter_keys: Option<Vec<String>> = args
        .key
        .as_deref()
        .map(|s| s.split(',').map(str::trim).map(str::to_string).collect());

    if args.ascii {
        println!("no data yet");
        return Ok(());
    }

    if let Some(keys) = &filter_keys {
        let metrics = store
            .list_metrics(&run.id, Some(keys))
            .context("failed to list metrics")?;

        if args.json {
            println!(
                "{}",
                serde_json::to_string(&metrics).unwrap_or_else(|_| "[]".to_string())
            );
        } else if metrics.is_empty() {
            println!("no metrics found for keys: {}", keys.join(", "));
        } else {
            println!("{:<8}  {:<30}  {:<12}  ts", "step", "key", "value");
            println!("{}", "-".repeat(70));
            for m in &metrics {
                println!(
                    "{:<8}  {:<30}  {:<12.6}  {}",
                    m.step,
                    m.key,
                    m.value,
                    m.ts.format("%Y-%m-%dT%H:%M:%SZ")
                );
            }
        }
    } else {
        let keys = store
            .list_metric_keys(&run.id)
            .context("failed to list metric keys")?;

        if args.json {
            let out: Vec<_> = keys
                .iter()
                .map(|(k, c)| serde_json::json!({"key": k, "count": c}))
                .collect();
            println!(
                "{}",
                serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string())
            );
        } else if keys.is_empty() {
            println!("no metrics for run {}", args.id);
        } else {
            println!("Available metric keys:");
            for (k, c) in &keys {
                println!("  {k}: {c} points");
            }
        }
    }

    Ok(())
}

/// Render metrics for the given keys to a 1200×600 PNG using plotters.
fn render_png(store: &Store, run_id: &RunId, keys: &[String], path: &Path) -> Result<()> {
    use plotters::prelude::*;

    const WIDTH: u32 = 1200;
    const HEIGHT: u32 = 600;

    // Palette: up to 8 distinguishable colours
    const PALETTE: &[RGBColor] = &[
        RGBColor(122, 162, 247), // blue
        RGBColor(158, 206, 106), // green
        RGBColor(247, 118, 142), // red
        RGBColor(224, 175, 104), // yellow
        RGBColor(187, 154, 247), // purple
        RGBColor(125, 207, 255), // cyan
        RGBColor(255, 158, 100), // orange
        RGBColor(150, 150, 150), // grey
    ];

    // Collect all series data
    let mut series: Vec<(String, Vec<(i64, f64)>)> = Vec::new();
    for key in keys {
        let pts = store
            .list_metrics(run_id, Some(std::slice::from_ref(key)))
            .context("failed to list metrics")?;
        if pts.is_empty() {
            continue;
        }
        let data: Vec<(i64, f64)> = pts.iter().map(|m| (m.step, m.value)).collect();
        series.push((key.clone(), data));
    }

    if series.is_empty() {
        bail!("no data points for the requested keys");
    }

    // Compute axis ranges
    let (x_min, x_max) = series
        .iter()
        .flat_map(|(_, d)| d.iter().map(|(x, _)| *x))
        .fold((i64::MAX, i64::MIN), |(mn, mx), v| (mn.min(v), mx.max(v)));
    let (y_min, y_max) = series
        .iter()
        .flat_map(|(_, d)| d.iter().map(|(_, y)| *y))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), v| {
            (mn.min(v), mx.max(v))
        });
    // Add 5% padding to y-axis
    let y_range = (y_max - y_min).max(1e-9);
    let y_lo = y_min - y_range * 0.05;
    let y_hi = y_max + y_range * 0.05;

    let root = BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area();
    root.fill(&RGBColor(26, 27, 38))?; // tokyo-night background

    let mut chart = ChartBuilder::on(&root)
        .caption(
            format!("run {}", &run_id.to_string()[..8]),
            ("sans-serif", 22).into_font().color(&WHITE),
        )
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(x_min..x_max, y_lo..y_hi)?;

    chart
        .configure_mesh()
        .x_desc("step")
        .axis_style(RGBColor(86, 95, 137))
        .label_style(("sans-serif", 14).into_font().color(&WHITE))
        .draw()?;

    for (i, (key, data)) in series.iter().enumerate() {
        let colour = PALETTE[i % PALETTE.len()];
        chart
            .draw_series(LineSeries::new(
                data.iter().copied(),
                colour.stroke_width(2),
            ))?
            .label(key.as_str())
            .legend(move |(x, y)| {
                PathElement::new(vec![(x, y), (x + 20, y)], colour.stroke_width(2))
            });
    }

    chart
        .configure_series_labels()
        .background_style(RGBColor(36, 40, 59).filled())
        .border_style(RGBColor(86, 95, 137))
        .label_font(("sans-serif", 14).into_font().color(&WHITE))
        .draw()?;

    root.present()?;
    Ok(())
}
