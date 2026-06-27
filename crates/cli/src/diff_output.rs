//! Helper to build compact, sortable directory names for manifest diff output.

/// Characters that are unsafe for filesystem paths and will be replaced with `_`.
const UNSAFE_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Maximum length for a sanitized label before truncation.
const MAX_LABEL_LEN: usize = 40;

/// Sanitize a label for use in a directory name: replace filesystem-unsafe
/// characters with `_` and truncate to `MAX_LABEL_LEN` chars.
fn sanitize_label(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .map(|c| if UNSAFE_CHARS.contains(&c) { '_' } else { c })
        .collect();
    if sanitized.len() > MAX_LABEL_LEN {
        sanitized[..MAX_LABEL_LEN].to_string()
    } else {
        sanitized
    }
}

/// Build a compact, sortable output directory path for manifest diffs.
///
/// The resulting path follows this convention:
/// `<data_dir>/exports/analysis/manifest-diffs/<timestamp-segment>__<old>-to-<new>/`
///
/// The timestamp segment is formatted differently depending on whether the two
/// timestamps fall on the same date:
/// - **Same date**: `<YYYYMMDD>_<HHMMSS>-<HHMMSS>`
/// - **Cross-date**: `<YYYYMMDD-HHMMSS>__<YYYYMMDD-HHMMSS>`
///
/// Both `old_label` and `new_label` are sanitized (filesystem-unsafe chars
/// replaced with `_`) and truncated to 40 characters.
///
/// # Arguments
/// * `data_dir` — Root data directory (e.g. `"/data"` or `"data"`).
/// * `old_label` — Human-readable label for the old manifest version.
/// * `new_label` — Human-readable label for the new manifest version.
/// * `old_time` — Timestamp for the old manifest in `YYYYMMDD-HHMMSS` format.
/// * `new_time` — Timestamp for the new manifest in `YYYYMMDD-HHMMSS` format.
pub fn build_diff_output_dir(
    data_dir: &str,
    old_label: &str,
    new_label: &str,
    old_time: &str,
    new_time: &str,
) -> String {
    let old_sanitized = sanitize_label(old_label);
    let new_sanitized = sanitize_label(new_label);

    let timestamp_segment =
        if old_time.len() >= 15 && new_time.len() >= 15 && old_time[..8] == new_time[..8] {
            // Same date: extract common date + individual HHMMSS parts.
            // old_time and new_time are "YYYYMMDD-HHMMSS" (15 chars).
            let date = &old_time[..8];
            let old_hms = &old_time[9..15];
            let new_hms = &new_time[9..15];
            format!("{date}_{old_hms}-{new_hms}")
        } else {
            // Cross-date or malformed timestamps: use full raw timestamps.
            format!("{old_time}__{new_time}")
        };

    format!(
        "{data_dir}/exports/analysis/manifest-diffs/{timestamp_segment}__{old_sanitized}-to-{new_sanitized}/"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_date_diff() {
        let result = build_diff_output_dir(
            "/data",
            "907578473352",
            "632191231927",
            "20260627-092000",
            "20260627-092106",
        );
        assert_eq!(
            result,
            "/data/exports/analysis/manifest-diffs/20260627_092000-092106__907578473352-to-632191231927/"
        );
    }

    #[test]
    fn cross_date_diff() {
        let result =
            build_diff_output_dir("/data", "old", "new", "20260627-235900", "20260628-000100");
        assert_eq!(
            result,
            "/data/exports/analysis/manifest-diffs/20260627-235900__20260628-000100__old-to-new/"
        );
    }

    #[test]
    fn label_sanitization() {
        let result = build_diff_output_dir(
            "/tmp",
            "a/b:c*d?e\"f<g>h|i",
            "normal",
            "20260627-120000",
            "20260628-120000",
        );
        let expected = "/tmp/exports/analysis/manifest-diffs/20260627-120000__20260628-120000__a_b_c_d_e_f_g_h_i-to-normal/";
        assert_eq!(result, expected);
    }

    #[test]
    fn label_truncation() {
        let long_label = "a".repeat(60);
        let result = build_diff_output_dir(
            "/data",
            &long_label,
            "short",
            "20260627-120000",
            "20260628-120000",
        );
        // The sanitized label should be truncated to 40 chars.
        let expected_label = "a".repeat(40);
        let expected = format!(
            "/data/exports/analysis/manifest-diffs/20260627-120000__20260628-120000__{expected_label}-to-short/"
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn malformed_timestamps_fallback_to_raw() {
        let result = build_diff_output_dir("/x", "old", "new", "bad", "also-bad");
        assert_eq!(
            result,
            "/x/exports/analysis/manifest-diffs/bad__also-bad__old-to-new/"
        );
    }
}
