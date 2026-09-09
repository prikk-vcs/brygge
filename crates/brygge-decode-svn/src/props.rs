//! SVN property classification (RFC 006 D-5): which properties map to IR file modes, which are refused,
//! and which are dropped-with-record. Also the `svn:date` → epoch-seconds conversion (no datetime
//! dependency — a civil-calendar computation, RFC 006 D-1 zero-dependency posture).

use crate::Error;

/// A borrowed property list (`&Props` and `&[..]` both coerce to this).
type PropSlice = [(String, Vec<u8>)];

/// `svn:executable` — the exec bit.
pub const SVN_EXECUTABLE: &str = "svn:executable";
/// `svn:special` — a symlink (content is `link <target>`).
pub const SVN_SPECIAL: &str = "svn:special";
/// `svn:mergeinfo` — advisory merge tracking (dropped-with-record, never a merge parent).
pub const SVN_MERGEINFO: &str = "svn:mergeinfo";
/// `svn:externals` — references other repositories (refused, INV-3).
pub const SVN_EXTERNALS: &str = "svn:externals";
/// `svn:author` — a revision's claimed author.
pub const SVN_AUTHOR: &str = "svn:author";
/// `svn:date` — a revision's claimed time (ISO 8601).
pub const SVN_DATE: &str = "svn:date";
/// `svn:log` — a revision's claimed message.
pub const SVN_LOG: &str = "svn:log";

const SVN_EOL_STYLE: &str = "svn:eol-style";
const SVN_KEYWORDS: &str = "svn:keywords";
const SVN_IGNORE: &str = "svn:ignore";
const SVN_GLOBAL_IGNORES: &str = "svn:global-ignores";

/// IR file mode for a regular file.
pub const MODE_REGULAR: u32 = 0o100_644;
/// IR file mode for an executable file.
pub const MODE_EXEC: u32 = 0o100_755;
/// IR file mode for a symlink.
pub const MODE_SYMLINK: u32 = 0o120_000;

/// Look up a property's raw value.
#[must_use]
pub fn get<'a>(props: &'a PropSlice, key: &str) -> Option<&'a [u8]> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_slice())
}

/// Look up a property's value as a UTF-8 string (absent, or non-UTF-8, → `None`).
#[must_use]
pub fn get_str(props: &PropSlice, key: &str) -> Option<String> {
    get(props, key)
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(str::to_string)
}

/// Whether a property is present.
#[must_use]
pub fn has(props: &PropSlice, key: &str) -> bool {
    props.iter().any(|(k, _)| k == key)
}

/// The IR file mode implied by a node's (complete) property set.
#[must_use]
pub fn file_mode(props: &PropSlice) -> u32 {
    if has(props, SVN_SPECIAL) {
        MODE_SYMLINK
    } else if has(props, SVN_EXECUTABLE) {
        MODE_EXEC
    } else {
        MODE_REGULAR
    }
}

/// An `svn:special` file stores `link <target>`; the IR symlink blob is the bare target.
#[must_use]
pub fn symlink_target(content: &[u8]) -> Vec<u8> {
    content
        .strip_prefix(b"link ")
        .map_or_else(|| content.to_vec(), <[u8]>::to_vec)
}

/// Refuse `svn:externals` (reaches other repositories — INV-3, the SVN analogue of submodules).
///
/// # Errors
/// [`Error::FloorRefusal`] when the property is present.
pub fn check_externals(node_path: &str, props: &PropSlice) -> Result<(), Error> {
    if has(props, SVN_EXTERNALS) {
        return Err(Error::FloorRefusal {
            feature: "svn:externals".to_string(),
            reason: format!(
                "svn:externals on '{node_path}' references other repositories (a network/trust \
                 surface, INV-3); refused rather than resolved (RFC 006 §4)"
            ),
        });
    }
    Ok(())
}

/// The loss categories a property set touches (for the loss boundary, RFC 006 D-5).
#[derive(Debug, Clone, Copy, Default)]
pub struct PropLoss {
    /// `svn:mergeinfo` seen (advisory-unreliable).
    pub mergeinfo: bool,
    /// A working-copy hint seen (`svn:eol-style`/`svn:keywords`/`svn:ignore`/`svn:global-ignores`).
    pub workflow: bool,
    /// A custom (non-recognized) property seen.
    pub custom: bool,
}

/// Classify a property set into loss categories (does not include the refused `svn:externals`, which is
/// handled by [`check_externals`], nor the mode-bearing `svn:executable`/`svn:special`).
#[must_use]
pub fn classify_loss(props: &PropSlice) -> PropLoss {
    let mut out = PropLoss::default();
    for (key, _) in props {
        match key.as_str() {
            SVN_MERGEINFO => out.mergeinfo = true,
            SVN_EOL_STYLE | SVN_KEYWORDS | SVN_IGNORE | SVN_GLOBAL_IGNORES => out.workflow = true,
            SVN_EXECUTABLE | SVN_SPECIAL | SVN_EXTERNALS => {}
            SVN_AUTHOR | SVN_DATE | SVN_LOG => {}
            _ => out.custom = true,
        }
    }
    out
}

/// Parse an `svn:date` (`YYYY-MM-DDThh:mm:ss[.ffffff]Z`) to epoch seconds. Returns `None` on any malformed
/// field — brygge carries no time rather than a wrong one (honest-over-guess).
#[must_use]
pub fn parse_svn_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let field = |start: usize, len: usize| -> Option<i64> {
        let slice = b.get(start..start.checked_add(len)?)?;
        std::str::from_utf8(slice).ok()?.parse::<i64>().ok()
    };
    let year = field(0, 4)?;
    let month = field(5, 2)?;
    let day = field(8, 2)?;
    let hour = field(11, 2)?;
    let min = field(14, 2)?;
    let sec = field(17, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days.checked_mul(86_400)? + hour * 3_600 + min * 60 + sec)
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + if m > 2 { -3 } else { 9 }) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests;
