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

        if args.per_key {
            render_png_grid(&store, &id, &keys_to_plot, png_path)
        } else {
            render_png(&store, &id, &keys_to_plot, png_path)
        }
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
            ("Arial", 22).into_font().color(&WHITE),
        )
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(x_min..x_max, y_lo..y_hi)?;

    chart
        .configure_mesh()
        .x_desc("step")
        .axis_style(RGBColor(86, 95, 137))
        .label_style(("Arial", 14).into_font().color(&WHITE))
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
        .label_font(("Arial", 14).into_font().color(&WHITE))
        .draw()?;

    root.present()?;
    Ok(())
}

/// Render one subplot per key in an auto-grid layout (each chart has its own
/// y-axis, so wildly different scales don't squash each other). PNG size grows
/// with key count: ~360×260 px per cell + margins.
fn render_png_grid(store: &Store, run_id: &RunId, keys: &[String], path: &Path) -> Result<()> {
    use plotters::prelude::*;

    // Tokyo-Night-friendly palette; index by the position of the key in the
    // input list so neighbouring cells get different colours.
    const PALETTE: &[RGBColor] = &[
        RGBColor(122, 162, 247),
        RGBColor(158, 206, 106),
        RGBColor(247, 118, 142),
        RGBColor(224, 175, 104),
        RGBColor(187, 154, 247),
        RGBColor(125, 207, 255),
        RGBColor(255, 158, 100),
        RGBColor(150, 150, 150),
    ];

    // Collect series, drop empty ones.
    let mut series: Vec<(String, Vec<(i64, f64)>)> = Vec::new();
    for key in keys {
        let pts = store
            .list_metrics(run_id, Some(std::slice::from_ref(key)))
            .context("failed to list metrics")?;
        if pts.is_empty() {
            continue;
        }
        series.push((key.clone(), pts.iter().map(|m| (m.step, m.value)).collect()));
    }
    if series.is_empty() {
        bail!("no data points for the requested keys");
    }

    let n = series.len();
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);

    const CELL_W: u32 = 360;
    const CELL_H: u32 = 260;
    const HEADER_H: u32 = 40;
    let width = (cols as u32) * CELL_W;
    let height = HEADER_H + (rows as u32) * CELL_H;

    let root = BitMapBackend::new(path, (width, height)).into_drawing_area();
    root.fill(&RGBColor(26, 27, 38))?;

    // Title strip at the top.
    let (title_area, body_area) = root.split_vertically(HEADER_H);
    title_area.titled(
        &format!("run {} — {} metrics", &run_id.to_string()[..8], n),
        ("Arial", 22).into_font().color(&WHITE),
    )?;

    let cells = body_area.split_evenly((rows, cols));
    for (i, ((key, data), area)) in series.iter().zip(cells.iter()).enumerate() {
        let colour = PALETTE[i % PALETTE.len()];

        let (x_min, x_max) = data
            .iter()
            .map(|(x, _)| *x)
            .fold((i64::MAX, i64::MIN), |(mn, mx), v| (mn.min(v), mx.max(v)));
        let x_max = if x_max == x_min { x_max + 1 } else { x_max };

        let (y_min, y_max) = data
            .iter()
            .map(|(_, y)| *y)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), v| {
                (mn.min(v), mx.max(v))
            });
        let y_range = (y_max - y_min).max(1e-9);
        let y_lo = y_min - y_range * 0.05;
        let y_hi = y_max + y_range * 0.05;

        let mut chart = ChartBuilder::on(area)
            .caption(key.as_str(), ("Arial", 14).into_font().color(&WHITE))
            .margin(8)
            .x_label_area_size(28)
            .y_label_area_size(48)
            .build_cartesian_2d(x_min..x_max, y_lo..y_hi)?;

        chart
            .configure_mesh()
            .axis_style(RGBColor(86, 95, 137))
            .label_style(("Arial", 11).into_font().color(&RGBColor(192, 202, 245)))
            .x_labels(4)
            .y_labels(4)
            .draw()?;

        chart.draw_series(LineSeries::new(
            data.iter().copied(),
            colour.stroke_width(2),
        ))?;

        // Last value as a small annotation in the top-right corner.
        if let Some((_, last)) = data.last() {
            let label = format!("last={last:.4}  n={}", data.len());
            chart.plotting_area().draw(&Text::new(
                label,
                (x_min + (x_max - x_min) / 25, y_hi - y_range * 0.08),
                ("Arial", 11).into_font().color(&RGBColor(86, 95, 137)),
            ))?;
        }
    }

    root.present()?;
    Ok(())
}
