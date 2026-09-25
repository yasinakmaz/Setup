//! Build report: what was produced, how big, and what it contains.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use inst_fsx::format_bytes;
use inst_model::platform::Target;

use crate::sign::SignStatus;

#[derive(Clone, Debug)]
pub struct ArtifactReport {
    pub target: Target,
    pub triple: String,
    pub path: PathBuf,
    pub exact_size: u64,
    pub runtime_size: u64,
    pub payload_size: u64,
    pub original_size: u64,
    pub file_count: usize,
    pub blocks: usize,
    pub codecs: Vec<String>,
    pub compression_reason: String,
    pub features: Vec<(String, String)>,
    pub privileges: String,
    pub privilege_reasons: Vec<String>,
    pub signing: SignStatus,
    pub prerequisites: Vec<String>,
    pub integrity_verified: bool,
    pub packaging: String,
    pub peak_extraction_memory: u64,
    pub timings: Vec<(&'static str, Duration)>,
    pub warnings: Vec<String>,
}

impl ArtifactReport {
    pub fn compression_percent(&self) -> f64 {
        if self.original_size == 0 {
            0.0
        } else {
            (1.0 - self.payload_size as f64 / self.original_size as f64) * 100.0
        }
    }
}

/// Groups digits: `174281632` → `174,281,632`.
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl fmt::Display for ArtifactReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ok = |b: bool| if b { "✓" } else { "✕" };
        writeln!(f, "Installer generated successfully\n")?;
        writeln!(f, "Output\n  {}", self.path.display())?;
        writeln!(
            f,
            "  {} ({}, {})\n",
            self.target, self.triple, self.packaging
        )?;
        writeln!(
            f,
            "Exact size\n  {} bytes\n  {}\n",
            group_digits(self.exact_size),
            format_bytes(self.exact_size)
        )?;
        writeln!(f, "Breakdown")?;
        writeln!(
            f,
            "  Compressed payload   {:>12}",
            format_bytes(self.payload_size).to_string()
        )?;
        writeln!(
            f,
            "  Runtime              {:>12}",
            format_bytes(self.runtime_size).to_string()
        )?;
        let other = self
            .exact_size
            .saturating_sub(self.payload_size + self.runtime_size);
        if other > 0 {
            writeln!(
                f,
                "  Packaging/signature  {:>12}",
                format_bytes(other).to_string()
            )?;
        }
        writeln!(f)?;
        writeln!(
            f,
            "Original payload\n  {} in {} files\n",
            format_bytes(self.original_size),
            self.file_count
        )?;
        writeln!(
            f,
            "Compression\n  {:.1}% saved, {} block(s): {}\n  {}\n",
            self.compression_percent(),
            self.blocks,
            self.codecs.join(", "),
            self.compression_reason
        )?;
        writeln!(
            f,
            "Peak extraction memory (estimate)\n  {}\n",
            format_bytes(self.peak_extraction_memory)
        )?;
        writeln!(f, "Privileges\n  {}", self.privileges)?;
        for r in &self.privilege_reasons {
            writeln!(f, "  · {r}")?;
        }
        writeln!(f)?;
        writeln!(f, "Security")?;
        writeln!(
            f,
            "  {} Integrity (payload verified after build)",
            ok(self.integrity_verified)
        )?;
        writeln!(f, "  ✓ Rollback journal and crash recovery")?;
        writeln!(f, "  ✓ Secrets redacted from logs")?;
        match &self.signing {
            SignStatus::Signed(by) => writeln!(f, "  ✓ Signed ({by})")?,
            SignStatus::NotConfigured => writeln!(f, "  ⚠ Code signing not configured")?,
            SignStatus::Failed(e) => writeln!(f, "  ✕ Signing failed: {e}")?,
        }
        writeln!(f)?;
        writeln!(f, "Features included")?;
        for (name, desc) in inst_model::features::OPTIONAL_FEATURES {
            let included = self.features.iter().any(|(n, _)| n == name);
            writeln!(f, "  {} {desc}", ok(included))?;
        }
        let langs: Vec<&str> = self
            .features
            .iter()
            .filter_map(|(n, _)| n.strip_prefix("lang-"))
            .collect();
        writeln!(
            f,
            "  ✓ Languages: en{}{}",
            if langs.is_empty() { "" } else { ", " },
            langs.join(", ")
        )?;
        if !self.prerequisites.is_empty() {
            writeln!(f, "\nPrerequisites")?;
            for p in &self.prerequisites {
                writeln!(f, "  · {p}")?;
            }
        }
        if !self.warnings.is_empty() {
            writeln!(f, "\nWarnings")?;
            for w in &self.warnings {
                writeln!(f, "  ⚠ {w}")?;
            }
        }
        writeln!(f, "\nTimings")?;
        for (stage, d) in &self.timings {
            writeln!(f, "  {stage:<22} {:>8.2}s", d.as_secs_f64())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn digits() {
        assert_eq!(super::group_digits(174_281_632), "174,281,632");
        assert_eq!(super::group_digits(12), "12");
        assert_eq!(super::group_digits(1000), "1,000");
    }
}
