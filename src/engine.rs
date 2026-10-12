use super::{
    Column, FitResult, Parameter, PlotStyle, Statistics, Workbook, column_code, column_label,
};
use calamine::{Reader, Xlsx};
use levenberg_marquardt::{LeastSquaresProblem, LevenbergMarquardt};
use nalgebra::{DMatrix, DVector, Dyn, Owned};
use serde::Deserialize;
use serde_json::{Value, json};
use statrs::distribution::{ContinuousCDF, StudentsT};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Request {
    action: String,
    #[serde(default)]
    workbook: Option<Workbook>,
    #[serde(default)]
    x_col: usize,
    #[serde(default)]
    y_col: usize,
    #[serde(default)]
    error_col: Option<usize>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    x_min: Option<f64>,
    #[serde(default)]
    x_max: Option<f64>,
    #[serde(default)]
    style: Option<PlotStyle>,
    #[serde(default)]
    fit: Option<FitResult>,
    #[serde(default)]
    path: PathBuf,
}

struct Control {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
impl Control {
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) || Instant::now() > self.deadline {
            Err("任务已停止或超过计算时限。".into())
        } else {
            Ok(())
        }
    }
}

pub(super) fn dispatch(value: Value, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    let r: Request = serde_json::from_value(value).map_err(|e| format!("任务参数有误：{e}"))?;
    let control = Control {
        cancel,
        deadline: Instant::now() + Duration::from_secs(90),
    };
    control.check()?;
    match r.action.as_str() {
        "import" => {
            serde_json::to_value(import_data(&r.path, &control)?).map_err(|e| e.to_string())
        }
        "fit" => serde_json::to_value(fit(&r, &control)?).map_err(|e| e.to_string()),
        "statistics" => serde_json::to_value(statistics(&r, &control)?).map_err(|e| e.to_string()),
        "export" => {
            export_graph(&r, &control)?;
            Ok(json!({"path":r.path}))
        }
        "export_csv" => {
            export_csv(&r, &control)?;
            Ok(json!({"path":r.path}))
        }
        _ => Err("未知的分析操作。".into()),
    }
}

fn split_header(value: &str, index: usize) -> Column {
    let value = value.trim();
    let mut name = if value.is_empty() {
        column_code(index)
    } else {
        value.into()
    };
    let mut unit = String::new();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if value.ends_with(close) {
            if let Some(pos) = value.rfind(open) {
                if pos > 0 {
                    name = value[..pos].trim().into();
                    unit = value[pos + 1..value.len() - 1].trim().into();
                    break;
                }
            }
        }
    }
    Column {
        name,
        unit,
        comments: String::new(),
    }
}

fn import_data(path: &Path, control: &Control) -> Result<Workbook, String> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("本原型支持 64 MB 以内的数据文件。".into());
    }
    let excel = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"));
    let mut records: Vec<Vec<String>> = vec![];
    if excel {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut total = 0_u64;
        for i in 0..zip.len() {
            total = total
                .checked_add(zip.by_index(i).map_err(|e| e.to_string())?.size())
                .ok_or("Excel 文件过大。")?;
        }
        if total > 128 * 1024 * 1024 {
            return Err("Excel 文件解压后过大。".into());
        }
        control.check()?;
        let mut excel: Xlsx<_> =
            calamine::open_workbook(path).map_err(|e: calamine::XlsxError| e.to_string())?;
        let range = excel
            .worksheet_range_at(0)
            .ok_or("Excel 中没有工作表。")?
            .map_err(|e| e.to_string())?;
        if range.height() > 50_001 || range.width() > 32 {
            return Err("工作表超过 50,000 行或 32 列。".into());
        }
        for row in range.rows() {
            control.check()?;
            records.push(row.iter().map(|v| v.to_string()).collect());
        }
    } else {
        let raw = std::fs::read(path).map_err(|e| e.to_string())?;
        let raw = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw);
        let text = match std::str::from_utf8(raw) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(_) => {
                let (s, errors) = encoding_rs::GB18030.decode_without_bom_handling(raw);
                if errors {
                    return Err("数据文件的字符编码有误。".into());
                }
                s
            }
        };
        let mut delimiter = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tsv"))
        {
            b'\t'
        } else {
            b','
        };
        let mut best = 1;
        for candidate in [b',', b'\t', b';'] {
            let first = csv::ReaderBuilder::new()
                .has_headers(false)
                .delimiter(candidate)
                .flexible(true)
                .from_reader(text.as_bytes())
                .records()
                .next();
            if let Some(Ok(record)) = first {
                if record.len() > best {
                    best = record.len();
                    delimiter = candidate;
                }
            }
        }
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(delimiter)
            .flexible(true)
            .trim(csv::Trim::All)
            .from_reader(text.as_bytes());
        for row in reader.records() {
            control.check()?;
            let row = row.map_err(|e| e.to_string())?;
            if records.len() > 50_000 {
                return Err("工作表超过 50,000 行。".into());
            }
            if row.len() > 32 {
                return Err("工作表超过 32 列。".into());
            }
            if row.iter().any(|s| !s.is_empty()) {
                records.push(row.iter().map(String::from).collect());
            }
        }
    }
    if records.is_empty() {
        return Err("文件中没有数据。".into());
    }
    let first = records.remove(0);
    let has_header = excel
        || first
            .iter()
            .any(|s| !s.trim().is_empty() && s.parse::<f64>().is_err());
    let headers = if has_header {
        first
    } else {
        let headers = (0..first.len()).map(column_code).collect();
        records.insert(0, first);
        headers
    };
    if headers.is_empty() || headers.len() > 32 || records.is_empty() || records.len() > 50_000 {
        return Err("工作表为空或超过容量限制。".into());
    }
    let columns: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(i, s)| split_header(s, i))
        .collect();
    if columns
        .iter()
        .any(|c| c.name.chars().count() > 512 || c.unit.chars().count() > 128)
    {
        return Err("列名称或单位过长。".into());
    }
    for (i, row) in records.iter_mut().enumerate() {
        if row.len() > columns.len() {
            return Err(format!("第 {} 行比标题多出数据列。", i + 2));
        }
        row.resize(columns.len(), String::new());
        for value in row {
            if value.chars().count() > 4096 {
                return Err("数据单元格过长。".into());
            }
            *value = value.trim().into();
        }
    }
    Ok(Workbook {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        columns,
        rows: records,
    })
}

struct Selection {
    x: Vec<f64>,
    y: Vec<f64>,
    sigma: Option<Vec<f64>>,
    dropped: usize,
}
fn selected(r: &Request, minimum: usize, control: &Control) -> Result<Selection, String> {
    let book = r.workbook.as_ref().ok_or("缺少工作表。")?;
    if book.rows.len() > 50_000 || book.columns.is_empty() || book.columns.len() > 32 {
        return Err("工作表超过容量限制。".into());
    }
    let columns: Vec<_> = [Some(r.x_col), Some(r.y_col), r.error_col]
        .into_iter()
        .flatten()
        .collect();
    if columns.iter().any(|i| *i >= book.columns.len()) {
        return Err("数据列选择有误。".into());
    }
    let range = match (r.x_min, r.x_max) {
        (None, None) => None,
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a < b => Some([a, b]),
        _ => return Err("分析区间有误。".into()),
    };
    let mut out = Selection {
        x: vec![],
        y: vec![],
        sigma: r.error_col.map(|_| vec![]),
        dropped: 0,
    };
    for (i, row) in book.rows.iter().enumerate() {
        control.check()?;
        if row.len() != book.columns.len() {
            return Err("工作表列结构有误。".into());
        }
        if columns.iter().any(|c| row[*c].trim().is_empty()) {
            out.dropped += 1;
            continue;
        }
        let numbers: Vec<_> = columns
            .iter()
            .map(|c| {
                row[*c]
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("第 {} 行的分析列含有非数值文本。", i + 1))
            })
            .collect::<Result<_, _>>()?;
        if numbers.iter().any(|n| !n.is_finite())
            || range.is_some_and(|[a, b]| numbers[0] < a || numbers[0] > b)
        {
            out.dropped += 1;
            continue;
        }
        if r.error_col.is_some() && numbers[2] <= 0.0 {
            return Err(format!("第 {} 行的标准差必须大于零。", i + 1));
        }
        out.x.push(numbers[0]);
        out.y.push(numbers[1]);
        if let Some(sigma) = &mut out.sigma {
            sigma.push(numbers[2]);
        }
    }
    if out.x.len() < minimum {
        return Err(format!("有效数据点不足，需要至少 {minimum} 个。"));
    }
    Ok(out)
}

fn evaluate(model: &str, p: &[f64], x: f64, x0: f64) -> f64 {
    match model {
        "linear" => p[0] * x + p[1],
        "quadratic" => p[0] * x * x + p[1] * x + p[2],
        "exponential" => p[0] + p[1] * (-(x - x0) / p[2]).exp(),
        _ => p[0] + p[1] * (-0.5 * ((x - p[2]) / p[3]).powi(2)).exp(),
    }
}

fn derivatives(model: &str, p: &[f64], x: f64, x0: f64) -> Vec<f64> {
    match model {
        "linear" => vec![x, 1.0],
        "quadratic" => vec![x * x, x, 1.0],
        "exponential" => {
            let e = (-(x - x0) / p[2]).exp();
            vec![1.0, e, p[1] * e * (x - x0) / p[2].powi(2)]
        }
        _ => {
            let d = x - p[2];
            let e = (-0.5 * (d / p[3]).powi(2)).exp();
            vec![
                1.0,
                e,
                p[1] * e * d / p[3].powi(2),
                p[1] * e * d * d / p[3].powi(3),
            ]
        }
    }
}

struct Nonlinear<'a> {
    data: &'a Selection,
    model: &'a str,
    q: DVector<f64>,
    x0: f64,
    span: f64,
    control: &'a Control,
}
impl Nonlinear<'_> {
    fn physical(&self) -> (Vec<f64>, Vec<f64>) {
        let minimum = (self.span * 1e-9).max(1e-12);
        if self.model == "exponential" {
            let d = self.q[2].exp();
            (vec![self.q[0], self.q[1], minimum + d], vec![1.0, 1.0, d])
        } else {
            let a = self.q[1].exp();
            let f = 1.0 / (1.0 + (-self.q[2]).exp());
            let w = self.q[3].exp();
            (
                vec![self.q[0], a, self.x0 + self.span * f, minimum + w],
                vec![1.0, a, self.span * f * (1.0 - f), w],
            )
        }
    }
}
impl LeastSquaresProblem<f64, Dyn, Dyn> for Nonlinear<'_> {
    type ParameterStorage = Owned<f64, Dyn>;
    type ResidualStorage = Owned<f64, Dyn>;
    type JacobianStorage = Owned<f64, Dyn, Dyn>;
    fn set_params(&mut self, p: &DVector<f64>) {
        self.q.copy_from(p);
    }
    fn params(&self) -> DVector<f64> {
        self.q.clone()
    }
    fn residuals(&self) -> Option<DVector<f64>> {
        if self.control.check().is_err() {
            return None;
        }
        let (p, _) = self.physical();
        if p.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let values = DVector::from_iterator(
            self.data.x.len(),
            self.data
                .x
                .iter()
                .zip(&self.data.y)
                .enumerate()
                .map(|(i, (x, y))| {
                    (evaluate(self.model, &p, *x, self.x0) - y)
                        / self.data.sigma.as_ref().map_or(1.0, |s| s[i])
                }),
        );
        values.iter().all(|v| v.is_finite()).then_some(values)
    }
    fn jacobian(&self) -> Option<DMatrix<f64>> {
        if self.control.check().is_err() {
            return None;
        }
        let (p, scale) = self.physical();
        let mut j = DMatrix::zeros(self.data.x.len(), p.len());
        for (i, x) in self.data.x.iter().enumerate() {
            let d = derivatives(self.model, &p, *x, self.x0);
            let sigma = self.data.sigma.as_ref().map_or(1.0, |s| s[i]);
            for k in 0..p.len() {
                j[(i, k)] = d[k] * scale[k] / sigma;
            }
        }
        j.iter().all(|v| v.is_finite()).then_some(j)
    }
}

fn fit(r: &Request, control: &Control) -> Result<FitResult, String> {
    let count = match r.model.as_str() {
        "linear" => 2,
        "quadratic" | "exponential" => 3,
        "gaussian" => 4,
        _ => return Err("请选择一个已支持的拟合模型。".into()),
    };
    let data = selected(r, count + 1, control)?;
    let xmin = data.x.iter().copied().fold(f64::INFINITY, f64::min);
    let xmax = data.x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = xmax - xmin;
    if span <= f64::EPSILON * xmin.abs().max(xmax.abs()).max(1.0) {
        return Err("X 列需要具有可辨别的数值范围。".into());
    }
    let (params, mut covariance) = if r.model == "linear" || r.model == "quadratic" {
        let mid = xmin + span / 2.0;
        let scale = span / 2.0;
        let design = DMatrix::from_fn(data.x.len(), count, |i, k| {
            ((data.x[i] - mid) / scale).powi((count - 1 - k) as i32)
                / data.sigma.as_ref().map_or(1.0, |s| s[i])
        });
        let svd = design.svd(true, true);
        let largest = svd.singular_values.max();
        if svd
            .singular_values
            .iter()
            .filter(|s| **s > largest * 1e-12)
            .count()
            < count
        {
            return Err("数据不足以确定全部拟合参数。".into());
        }
        let inverse = svd.pseudo_inverse(largest * 1e-12).map_err(String::from)?;
        let target = DVector::from_iterator(
            data.y.len(),
            data.y
                .iter()
                .enumerate()
                .map(|(i, y)| y / data.sigma.as_ref().map_or(1.0, |s| s[i])),
        );
        let q = &inverse * target;
        let transform = if count == 2 {
            DMatrix::from_row_slice(2, 2, &[1.0 / scale, 0.0, -mid / scale, 1.0])
        } else {
            DMatrix::from_row_slice(
                3,
                3,
                &[
                    1.0 / scale.powi(2),
                    0.0,
                    0.0,
                    -2.0 * mid / scale.powi(2),
                    1.0 / scale,
                    0.0,
                    mid.powi(2) / scale.powi(2),
                    -mid / scale,
                    1.0,
                ],
            )
        };
        let params = &transform * q;
        let cov = &transform * (&inverse * inverse.transpose()) * transform.transpose();
        (params.as_slice().to_vec(), Some(cov))
    } else {
        let min_y = data.y.iter().copied().fold(f64::INFINITY, f64::min);
        let max_y = data.y.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let q = if r.model == "exponential" {
            let first = data.x.iter().position(|v| *v == xmin).unwrap();
            let last = data.x.iter().position(|v| *v == xmax).unwrap();
            DVector::from_vec(vec![
                data.y[last],
                data.y[first] - data.y[last],
                (span / 3.0).ln(),
            ])
        } else {
            let peak = data.y.iter().position(|v| *v == max_y).unwrap();
            let fraction = ((data.x[peak] - xmin) / span).clamp(1e-6, 1.0 - 1e-6);
            DVector::from_vec(vec![
                min_y,
                (max_y - min_y).max(1e-9).ln(),
                (fraction / (1.0 - fraction)).ln(),
                (span / 8.0).ln(),
            ])
        };
        let problem = Nonlinear {
            data: &data,
            model: &r.model,
            q,
            x0: xmin,
            span,
            control,
        };
        let (solution, report) = LevenbergMarquardt::new()
            .with_patience(200)
            .minimize(problem);
        control.check()?;
        if !report.termination.was_successful() {
            return Err(format!("拟合收敛失败：{:?}", report.termination));
        }
        let (params, _) = solution.physical();
        let mut design = DMatrix::zeros(data.x.len(), count);
        for (i, x) in data.x.iter().enumerate() {
            let d = derivatives(&r.model, &params, *x, xmin);
            for k in 0..count {
                design[(i, k)] = d[k] / data.sigma.as_ref().map_or(1.0, |s| s[i]);
            }
        }
        let svd = design.svd(true, true);
        let largest = svd.singular_values.max();
        let cov = if svd
            .singular_values
            .iter()
            .filter(|s| **s > largest * 1e-12)
            .count()
            == count
        {
            let inv = svd.pseudo_inverse(largest * 1e-12).map_err(String::from)?;
            Some(&inv * inv.transpose())
        } else {
            None
        };
        (params, cov)
    };
    if params.iter().any(|p| !p.is_finite()) {
        return Err("拟合参数超出数值范围。".into());
    }
    let residuals: Vec<_> = data
        .x
        .iter()
        .zip(&data.y)
        .map(|(x, y)| [*x, y - evaluate(&r.model, &params, *x, xmin)])
        .collect();
    let rss = residuals.iter().map(|r| r[1] * r[1]).sum::<f64>();
    let weighted_rss = residuals
        .iter()
        .enumerate()
        .map(|(i, r)| (r[1] / data.sigma.as_ref().map_or(1.0, |s| s[i])).powi(2))
        .sum::<f64>();
    if data.sigma.is_none() {
        if let Some(c) = &mut covariance {
            *c *= weighted_rss / (data.x.len() - count) as f64;
        }
    }
    let critical = StudentsT::new(0.0, 1.0, (data.x.len() - count) as f64)
        .map_err(|e| e.to_string())?
        .inverse_cdf(0.975);
    let names: Vec<&str> = match r.model.as_str() {
        "linear" => vec!["a", "b"],
        "quadratic" => vec!["a", "b", "c"],
        "exponential" => vec!["y0", "A", "tau"],
        _ => vec!["y0", "A", "xc", "sigma"],
    };
    let output = params
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let error = covariance
                .as_ref()
                .map(|c| c[(i, i)].max(0.0).sqrt())
                .filter(|e| e.is_finite());
            Parameter {
                name: names[i].into(),
                value: *value,
                stderr: error,
                low: error.map(|e| value - critical * e),
                high: error.map(|e| value + critical * e),
            }
        })
        .collect();
    let mean = data.y.iter().sum::<f64>() / data.y.len() as f64;
    let total = data.y.iter().map(|y| (y - mean).powi(2)).sum::<f64>();
    let points: Vec<_> = (0..360)
        .map(|i| {
            let x = xmin + span * i as f64 / 359.0;
            [x, evaluate(&r.model, &params, x, xmin)]
        })
        .collect();
    if !rss.is_finite() || points.iter().flatten().any(|v| !v.is_finite()) {
        return Err("拟合结果超出数值范围。".into());
    }
    let equation = match r.model.as_str() {
        "linear" => "y = a*x + b".into(),
        "quadratic" => "y = a*x² + b*x + c".into(),
        "exponential" => format!("y = y0 + A*exp(-(x - {xmin})/tau)"),
        _ => "y = y0 + A*exp(-(x-xc)²/(2*sigma²))".into(),
    };
    Ok(FitResult {
        model: r.model.clone(),
        equation,
        params: output,
        points,
        residuals,
        r2: (total > 0.0).then_some(1.0 - rss / total),
        rmse: (rss / data.y.len() as f64).sqrt(),
        n: data.x.len(),
        dropped: data.dropped,
        weighted: data.sigma.is_some(),
    })
}

fn statistics(r: &Request, control: &Control) -> Result<Statistics, String> {
    let mut data = selected(r, 1, control)?;
    let count = data.y.len();
    let mut mean = 0.0;
    let mut sum2 = 0.0;
    for (i, y) in data.y.iter().enumerate() {
        let delta = y - mean;
        mean += delta / (i + 1) as f64;
        sum2 += delta * (y - mean);
    }
    data.y.sort_by(f64::total_cmp);
    let median = if count % 2 == 0 {
        data.y[count / 2 - 1] + (data.y[count / 2] - data.y[count / 2 - 1]) / 2.0
    } else {
        data.y[count / 2]
    };
    Ok(Statistics {
        count,
        dropped: data.dropped,
        mean,
        median,
        sd: (count > 1).then(|| (sum2 / (count - 1) as f64).sqrt()),
        minimum: data.y[0],
        maximum: data.y[count - 1],
    })
}

fn atomic_output(path: &Path, bytes: &[u8], control: &Control) -> Result<(), String> {
    use std::sync::atomic::AtomicU64;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    control.check()?;
    let temporary = path.with_file_name(format!(
        ".plotlab-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        control.check()?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn export_csv(r: &Request, control: &Control) -> Result<(), String> {
    let book = r.workbook.as_ref().ok_or("缺少工作表。")?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    let headers: Vec<_> = book
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if c.name.is_empty() {
                column_code(i)
            } else {
                column_label(c)
            }
        })
        .collect();
    writer.write_record(headers).map_err(|e| e.to_string())?;
    for row in &book.rows {
        control.check()?;
        writer.write_record(row).map_err(|e| e.to_string())?;
    }
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(writer.into_inner().map_err(|e| e.to_string())?);
    atomic_output(&r.path, &bytes, control)
}

fn export_graph(r: &Request, control: &Control) -> Result<(), String> {
    use plotters::prelude::*;
    let extension = r
        .path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if !["svg", "pdf", "png"].contains(&extension.as_str()) {
        return Err("请选择 SVG、PDF 或 PNG 格式。".into());
    }
    let data = selected(r, 2, control)?;
    let style = r.style.as_ref().ok_or("缺少图形属性。")?;
    if !style.width.is_finite() || !style.marker_size.is_finite() {
        return Err("图形样式有误。".into());
    }
    let mut xy: Vec<_> = data.x.iter().copied().zip(data.y.iter().copied()).collect();
    xy.sort_by(|a, b| a.0.total_cmp(&b.0));
    let x0 = xy[0].0;
    let x1 = xy.last().unwrap().0;
    let mut y0 = f64::INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for (i, y) in data.y.iter().enumerate() {
        let error = data.sigma.as_ref().map_or(0.0, |s| s[i]);
        y0 = y0.min(y - error);
        y1 = y1.max(y + error);
    }
    if let Some(fit) = &r.fit {
        for p in &fit.points {
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
    }
    let xmargin = ((x1 - x0) * 0.04).max(if x1 == x0 { 1.0 } else { f64::EPSILON });
    let ymargin = ((y1 - y0) * 0.06).max(if y1 == y0 { 1.0 } else { f64::EPSILON });
    if ![x0 - xmargin, x1 + xmargin, y0 - ymargin, y1 + ymargin]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err("绘图数据超出数值范围。".into());
    }
    let color = RGBColor(style.color[0], style.color[1], style.color[2]);
    let stroke =
        ShapeStyle::from(&color).stroke_width((style.width * 300.0 / 72.0).round().max(1.0) as u32);
    let radius = (style.marker_size * 300.0 / 72.0 / 2.0).round().max(1.0) as i32;
    let book = r.workbook.as_ref().unwrap();
    let label = if book.columns[r.y_col].name.is_empty() {
        column_code(r.y_col)
    } else {
        book.columns[r.y_col].name.clone()
    };
    let mut svg = String::new();
    let draw_result: Result<(), String> = (|| {
        let root = SVGBackend::with_string(&mut svg, (2160, 1500)).into_drawing_area();
        root.fill(&WHITE).map_err(|e| e.to_string())?;
        let mut builder = ChartBuilder::on(&root);
        builder
            .margin(60)
            .x_label_area_size(165)
            .y_label_area_size(195);
        if !style.title.is_empty() {
            builder.caption(&style.title, ("Microsoft YaHei", 66));
        }
        let mut chart = builder
            .build_cartesian_2d(x0 - xmargin..x1 + xmargin, y0 - ymargin..y1 + ymargin)
            .map_err(|e| e.to_string())?;
        let mut mesh = chart.configure_mesh();
        mesh.x_desc(&style.x_label)
            .y_desc(&style.y_label)
            .axis_desc_style(("Microsoft YaHei", 48))
            .label_style(("Arial", 42))
            .axis_style(BLACK.stroke_width(3))
            .set_tick_mark_size(LabelAreaPosition::Bottom, 15)
            .set_tick_mark_size(LabelAreaPosition::Left, 15)
            .light_line_style(RGBColor(225, 225, 225));
        if !style.grid {
            mesh.disable_mesh();
        }
        mesh.draw().map_err(|e| e.to_string())?;
        if style.kind != "scatter" {
            chart
                .draw_series(LineSeries::new(xy.clone(), stroke))
                .map_err(|e| e.to_string())?
                .label(&label)
                .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 66, y)], stroke));
        }
        if style.kind != "line" {
            let series = chart
                .draw_series(xy.iter().map(|p| Circle::new(*p, radius, color.filled())))
                .map_err(|e| e.to_string())?;
            if style.kind == "scatter" {
                series
                    .label(&label)
                    .legend(move |(x, y)| Circle::new((x + 30, y), radius, color.filled()));
            }
        }
        if let Some(sigma) = &data.sigma {
            chart
                .draw_series(data.x.iter().zip(&data.y).zip(sigma).map(|((x, y), e)| {
                    PathElement::new(
                        vec![(*x, y - e), (*x, y + e)],
                        color.mix(0.6).stroke_width(1),
                    )
                }))
                .map_err(|e| e.to_string())?;
        }
        if let Some(fit) = &r.fit {
            chart
                .draw_series(LineSeries::new(
                    fit.points.iter().map(|p| (p[0], p[1])),
                    RGBColor(206, 54, 52).stroke_width(9),
                ))
                .map_err(|e| e.to_string())?
                .label("Fit")
                .legend(|(x, y)| {
                    PathElement::new(
                        vec![(x, y), (x + 66, y)],
                        RGBColor(206, 54, 52).stroke_width(9),
                    )
                });
        }
        if style.legend {
            chart
                .configure_series_labels()
                .legend_area_size(90)
                .position(SeriesLabelPosition::UpperRight)
                .label_font(("Arial", 45))
                .background_style(WHITE.mix(0.85))
                .draw()
                .map_err(|e| e.to_string())?;
        }
        root.present().map_err(|e| e.to_string())?;
        Ok(())
    })();
    draw_result?;
    control.check()?;
    if extension == "svg" {
        return atomic_output(&r.path, svg.as_bytes(), control);
    }
    let mut options = svg2pdf::usvg::Options::default();
    options.dpi = 300.0;
    options.font_family = "Microsoft YaHei".into();
    let db = options.fontdb_mut();
    let arial = db.load_font_file("C:/Windows/Fonts/arial.ttf");
    let cjk = db.load_font_file("C:/Windows/Fonts/msyh.ttc");
    if arial.is_err() && cjk.is_err() {
        db.load_system_fonts();
    }
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options).map_err(|e| e.to_string())?;
    control.check()?;
    if extension == "pdf" {
        let bytes = svg2pdf::to_pdf(
            &tree,
            svg2pdf::ConversionOptions::default(),
            svg2pdf::PageOptions { dpi: 300.0 },
        )
        .map_err(|e| e.to_string())?;
        atomic_output(&r.path, &bytes, control)
    } else {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(2160, 1500).ok_or("图像分配失败。")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2160, 1500);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: 11811,
                yppu: 11811,
                unit: png::Unit::Meter,
            }));
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer
                .write_image_data(pixmap.data())
                .map_err(|e| e.to_string())?;
        }
        atomic_output(&r.path, &bytes, control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn control() -> Control {
        Control {
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + Duration::from_secs(20),
        }
    }
    fn request(model: &str) -> Request {
        serde_json::from_value(super::super::Project::demo(model).request("fit")).unwrap()
    }
    #[test]
    fn fits_match_scipy_reference_parameters_and_errors() {
        let reference: Value =
            serde_json::from_str(include_str!("../tests/fixtures/scipy-reference.json")).unwrap();
        for case in reference["cases"].as_array().unwrap() {
            let r: Request = serde_json::from_value(case["request"].clone()).unwrap();
            let result = fit(&r, &control()).unwrap();
            for (actual, expected) in result
                .params
                .iter()
                .zip(case["expected"]["params"].as_array().unwrap())
            {
                let value = expected["value"].as_f64().unwrap();
                assert!(
                    (actual.value - value).abs() < value.abs().max(1.0) * 2e-5,
                    "{} {}: {} != {}",
                    r.model,
                    actual.name,
                    actual.value,
                    value
                );
                let error = expected["stderr"].as_f64().unwrap();
                assert!(
                    (actual.stderr.unwrap() - error).abs() < error.abs().max(1e-5) * 2e-3,
                    "{} stderr {}",
                    r.model,
                    actual.name
                );
            }
            let rmse = case["expected"]["rmse"].as_f64().unwrap();
            assert!((result.rmse - rmse).abs() < 1e-7);
        }
    }
    #[test]
    fn analytic_jacobians_match_finite_differences() {
        for (model, p, x0, x) in [
            ("exponential", vec![5.0, 60.0, 3.1], 0.0, 2.5),
            ("gaussian", vec![8.0, 62.0, 1200.0, 72.0], 800.0, 1250.0),
        ] {
            let derivatives = derivatives(model, &p, x, x0);
            for i in 0..p.len() {
                let step = p[i].abs().max(1.0) * 1e-6;
                let mut a = p.clone();
                let mut b = p.clone();
                a[i] += step;
                b[i] -= step;
                let numeric =
                    (evaluate(model, &a, x, x0) - evaluate(model, &b, x, x0)) / (2.0 * step);
                assert!((derivatives[i] - numeric).abs() < numeric.abs().max(1.0) * 1e-6);
            }
        }
    }
    #[test]
    fn missing_values_weights_and_bad_input_are_handled() {
        let mut r = request("linear");
        r.workbook.as_mut().unwrap().rows[0][1].clear();
        r.workbook.as_mut().unwrap().rows[1][1] = "NaN".into();
        assert_eq!(fit(&r, &control()).unwrap().dropped, 2);
        r.error_col = Some(3);
        r.workbook.as_mut().unwrap().rows[2][3] = "0".into();
        assert!(fit(&r, &control()).unwrap_err().contains("标准差"));
        let mut r = request("linear");
        for row in &mut r.workbook.as_mut().unwrap().rows {
            row[0] = "1".into();
        }
        assert!(fit(&r, &control()).is_err());
        let mut r = request("linear");
        for row in &mut r.workbook.as_mut().unwrap().rows {
            row[1] = "4".into();
        }
        assert!(fit(&r, &control()).unwrap().r2.is_none());
    }
    #[test]
    fn statistics_and_cancel_are_correct() {
        let mut r = request("linear");
        let b = r.workbook.as_mut().unwrap();
        b.rows = vec![vec!["0".into(), "3".into(), "".into(), "".into()]];
        let stats = statistics(&r, &control()).unwrap();
        assert_eq!(stats.mean, 3.0);
        assert_eq!(stats.sd, None);
        let c = control();
        c.cancel.store(true, Ordering::Relaxed);
        assert!(fit(&r, &c).is_err());
    }
    #[test]
    fn xlsx_and_csv_round_trip_preserve_values() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import.xlsx");
        let book = import_data(&path, &control()).unwrap();
        assert_eq!(book.rows[0], vec!["1", "2"]);
        assert_eq!(book.columns[0].unit, "s");
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("verification/rust-tests");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("precision.csv");
        std::fs::write(
            &path,
            "\u{feff}\"Time (s)\",\"Signal, sample (mV)\"\n1.123456789012345,-2.5\n2,3\n",
        )
        .unwrap();
        let book = import_data(&path, &control()).unwrap();
        assert_eq!(book.rows[0][0], "1.123456789012345");
        assert_eq!(book.columns[1].name, "Signal, sample");
        let mut r = request("linear");
        r.workbook = Some(book.clone());
        r.path = folder.join("roundtrip.csv");
        export_csv(&r, &control()).unwrap();
        assert_eq!(import_data(&r.path, &control()).unwrap().rows, book.rows);
        std::fs::write(&path, "x,y\n1,2,3\n").unwrap();
        assert!(import_data(&path, &control()).is_err());
    }
    #[test]
    fn native_export_formats_are_valid() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("verification/rust-tests");
        std::fs::create_dir_all(&folder).unwrap();
        let mut r = request("exponential");
        r.fit = Some(fit(&r, &control()).unwrap());
        for extension in ["svg", "pdf", "png"] {
            r.path = folder.join(format!("native.{extension}"));
            export_graph(&r, &control()).unwrap();
            let bytes = std::fs::read(&r.path).unwrap();
            assert!(bytes.len() > 1000);
            match extension {
                "svg" => assert!(String::from_utf8(bytes).unwrap().contains("<svg")),
                "pdf" => assert!(bytes.starts_with(b"%PDF-")),
                _ => {
                    let image = image::load_from_memory(&bytes).unwrap();
                    assert_eq!((image.width(), image.height()), (2160, 1500));
                }
            }
        }
    }
}
