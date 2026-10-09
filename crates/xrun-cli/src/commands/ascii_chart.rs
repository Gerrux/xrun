#![deny(unsafe_code)]

//! Детерминированный ASCII-график «значение от шага» для `xrun metrics --ascii`.
//! Только печатные ASCII-символы (без braille / box-drawing): Windows-консоль
//! с кодовой страницей OEM их ломает, а тесты сравнивают подстроки.

/// Ширина области графика в колонках (без оси Y).
pub const CHART_WIDTH: usize = 60;
/// Высота области графика в строках.
pub const CHART_HEIGHT: usize = 8;

/// Ширина подписи оси Y (например `  0.9876 |`).
const LABEL_WIDTH: usize = 10;

/// Отрисовать серию `points` (шаг, значение) в фиксированную сетку
/// [`CHART_WIDTH`]×[`CHART_HEIGHT`]. Нечисловые значения (NaN / ±inf)
/// отбрасываются; если после фильтрации точек нет — возвращается `None`.
///
/// Серии длиннее ширины сжимаются усреднением по корзинам, короче —
/// растягиваются (одна точка на колонку, промежутки пустые). Соседние
/// колонки соединяются вертикальными штрихами, чтобы читалось как линия.
pub fn render_ascii(key: &str, points: &[(i64, f64)]) -> Option<String> {
    let finite: Vec<(i64, f64)> = points
        .iter()
        .copied()
        .filter(|(_, v)| v.is_finite())
        .collect();
    if finite.is_empty() {
        return None;
    }
    let dropped = points.len() - finite.len();

    let (min, max) = finite
        .iter()
        .map(|(_, v)| *v)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), v| {
            (mn.min(v), mx.max(v))
        });
    let last = finite.last().map(|(_, v)| *v).unwrap_or(min);
    let first_step = finite.first().map(|(s, _)| *s).unwrap_or(0);
    let last_step = finite.last().map(|(s, _)| *s).unwrap_or(first_step);

    // Колонки: усреднённое значение корзины или `None` для пустой колонки.
    let columns = bucketize(&finite, CHART_WIDTH);

    // Отображение значения в индекс строки: 0 — верх (max), HEIGHT-1 — низ (min).
    let range = (max - min).max(1e-12);
    let row_of = |v: f64| -> usize {
        let frac = ((v - min) / range).clamp(0.0, 1.0);
        let r = ((1.0 - frac) * (CHART_HEIGHT - 1) as f64).round() as usize;
        r.min(CHART_HEIGHT - 1)
    };

    let mut grid = vec![vec![' '; CHART_WIDTH]; CHART_HEIGHT];
    let mut prev_row: Option<usize> = None;
    for (col, cell) in columns.iter().enumerate() {
        let Some(v) = cell else {
            prev_row = None;
            continue;
        };
        let row = row_of(*v);
        if let Some(pr) = prev_row {
            // Вертикальная связка между соседними колонками (без самих узлов).
            let (lo, hi) = if pr < row { (pr, row) } else { (row, pr) };
            for r in grid.iter_mut().take(hi).skip(lo + 1) {
                if r[col] == ' ' {
                    r[col] = '|';
                }
            }
        }
        grid[row][col] = '*';
        prev_row = Some(row);
    }

    let mut out = String::new();
    out.push_str(&format!("{key}\n"));
    for (r, row) in grid.iter().enumerate() {
        let label = if r == 0 {
            fmt_label(max)
        } else if r == CHART_HEIGHT - 1 {
            fmt_label(min)
        } else if r == CHART_HEIGHT / 2 {
            fmt_label(min + range / 2.0)
        } else {
            String::new()
        };
        let line: String = row.iter().collect();
        out.push_str(&format!(
            "{label:>width$} |{line}\n",
            width = LABEL_WIDTH - 2
        ));
    }
    out.push_str(&format!(
        "{:>width$} +{}\n",
        "",
        "-".repeat(CHART_WIDTH),
        width = LABEL_WIDTH - 2
    ));
    out.push_str(&format!(
        "{:>width$}  step {first_step}{:>pad$}{last_step}\n",
        "",
        "",
        width = LABEL_WIDTH - 2,
        // «step » + первый шаг + заполнитель + последний шаг = ровно CHART_WIDTH,
        // чтобы последний шаг заканчивался под правым краем графика.
        pad = CHART_WIDTH
            .saturating_sub(5 + first_step.to_string().len() + last_step.to_string().len()),
    ));
    out.push_str(&format!(
        "{:>width$}  min={min:.4}  max={max:.4}  last={last:.4}  n={}",
        "",
        finite.len(),
        width = LABEL_WIDTH - 2
    ));
    if dropped > 0 {
        out.push_str(&format!("  (skipped {dropped} non-finite)"));
    }
    out.push('\n');
    Some(out)
}

/// Разложить упорядоченные точки по `width` колонкам. Если точек не больше
/// ширины, каждая занимает свою колонку пропорционально позиции в серии;
/// иначе точки усредняются внутри корзины.
fn bucketize(points: &[(i64, f64)], width: usize) -> Vec<Option<f64>> {
    let n = points.len();
    let mut cols: Vec<Option<f64>> = vec![None; width];
    if n == 1 {
        cols[0] = Some(points[0].1);
        return cols;
    }
    if n <= width {
        for (i, (_, v)) in points.iter().enumerate() {
            let col = i * (width - 1) / (n - 1);
            cols[col] = Some(*v);
        }
        return cols;
    }
    let mut sums = vec![0.0f64; width];
    let mut counts = vec![0usize; width];
    for (i, (_, v)) in points.iter().enumerate() {
        let col = (i * width / n).min(width - 1);
        sums[col] += v;
        counts[col] += 1;
    }
    for col in 0..width {
        if counts[col] > 0 {
            cols[col] = Some(sums[col] / counts[col] as f64);
        }
    }
    cols
}

fn fmt_label(v: f64) -> String {
    let abs = v.abs();
    if abs != 0.0 && (abs >= 1e6 || abs < 1e-3) {
        format!("{v:.2e}")
    } else {
        format!("{v:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_series_is_none() {
        assert!(render_ascii("k", &[]).is_none());
        assert!(render_ascii("k", &[(0, f64::NAN), (1, f64::INFINITY)]).is_none());
    }

    #[test]
    fn single_point_renders_without_panic() {
        let out = render_ascii("loss", &[(5, 0.5)]).unwrap();
        assert!(out.starts_with("loss\n"));
        assert!(out.contains("min=0.5000  max=0.5000  last=0.5000  n=1"));
        assert_eq!(out.matches('*').count(), 1);
    }

    #[test]
    fn constant_series_does_not_divide_by_zero() {
        let pts: Vec<(i64, f64)> = (0..10).map(|s| (s, 1.0)).collect();
        let out = render_ascii("acc", &pts).unwrap();
        assert_eq!(out.matches('*').count(), 10);
        assert!(out.contains("n=10"));
    }

    #[test]
    fn series_longer_than_width_is_bucketed() {
        let pts: Vec<(i64, f64)> = (0..500).map(|s| (s, s as f64)).collect();
        let out = render_ascii("step", &pts).unwrap();
        // Каждая колонка занята ровно одним узлом.
        assert_eq!(out.matches('*').count(), CHART_WIDTH);
        assert!(out.contains("min=0.0000  max=499.0000  last=499.0000  n=500"));
        assert!(out.contains("step 0"));
        assert!(out.contains("499\n"));
    }

    #[test]
    fn non_finite_points_are_skipped_and_reported() {
        let pts = [(0, 1.0), (1, f64::NAN), (2, 3.0)];
        let out = render_ascii("v", &pts).unwrap();
        assert!(out.contains("n=2"));
        assert!(out.contains("(skipped 1 non-finite)"));
        assert!(out.contains("max=3.0000"));
    }

    #[test]
    fn grid_has_fixed_dimensions() {
        let pts: Vec<(i64, f64)> = (0..7).map(|s| (s, (s as f64).sin())).collect();
        let out = render_ascii("sin", &pts).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // Заголовок + HEIGHT строк + ось + подпись шагов + статистика.
        assert_eq!(lines.len(), 1 + CHART_HEIGHT + 3);
        for line in &lines[1..=CHART_HEIGHT] {
            assert_eq!(line.len(), LABEL_WIDTH + CHART_WIDTH, "{line:?}");
        }
    }
}
