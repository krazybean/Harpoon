//! Static checks on the build input, done before any build starts. Pure.
//!
//! Harpoon's first slice builds from **local** base images only: no pulls, no external
//! frontends (`# syntax=` pulls a frontend image), no variable base images.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerfileProblem {
    /// A `# syntax=` parser directive would make BuildKit pull a frontend image.
    ExternalFrontend(String),
    /// `FROM $VAR` / `FROM ${VAR}`: the image cannot be checked locally.
    VariableImage(String),
    Empty,
}

impl std::fmt::Display for DockerfileProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerfileProblem::ExternalFrontend(l) => write!(
                f,
                "the Dockerfile requests an external frontend ({l:?}); Harpoon builds with the built-in frontend only, because the frontend image would have to be pulled"
            ),
            DockerfileProblem::VariableImage(l) => write!(
                f,
                "the base image in {l:?} is a variable, so Harpoon cannot confirm it exists locally; write the image name literally"
            ),
            DockerfileProblem::Empty => write!(f, "the Dockerfile has no FROM instruction"),
        }
    }
}

/// External images the build will need: every `FROM <image>` and `COPY --from=<image>` that is
/// not `scratch`, an earlier stage name or a stage number.
pub fn required_images(text: &str) -> Result<BTreeSet<String>, DockerfileProblem> {
    let mut stages: BTreeSet<String> = BTreeSet::new();
    let mut images = BTreeSet::new();
    let mut seen_from = false;
    let mut in_directives = true;
    let mut stage_count = 0usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            in_directives = false;
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            let c = c.trim();
            let lower = c.to_ascii_lowercase();
            // Parser directives are `# key=value` before any instruction or blank line; be strict
            // and reject a syntax directive anywhere in the leading comment block.
            if in_directives && lower.replace(' ', "").starts_with("syntax=") {
                return Err(DockerfileProblem::ExternalFrontend(line.to_string()));
            }
            continue;
        }
        in_directives = false;
        let mut words = line.split_whitespace();
        let Some(op) = words.next() else { continue };
        match op.to_ascii_uppercase().as_str() {
            "FROM" => {
                seen_from = true;
                let rest: Vec<&str> = words.filter(|w| !w.starts_with("--")).collect();
                let Some(image) = rest.first() else { continue };
                if image.contains('$') {
                    return Err(DockerfileProblem::VariableImage(line.to_string()));
                }
                if !image.eq_ignore_ascii_case("scratch")
                    && !stages.contains(&image.to_ascii_lowercase())
                {
                    images.insert((*image).to_string());
                }
                if rest.len() >= 3 && rest[1].eq_ignore_ascii_case("as") {
                    stages.insert(rest[2].to_ascii_lowercase());
                }
                stage_count += 1;
            }
            "COPY" | "ADD" => {
                for w in line.split_whitespace().skip(1) {
                    if let Some(from) = w.strip_prefix("--from=") {
                        if from.contains('$') {
                            return Err(DockerfileProblem::VariableImage(line.to_string()));
                        }
                        let is_stage = stages.contains(&from.to_ascii_lowercase())
                            || from
                                .parse::<usize>()
                                .map(|n| n < stage_count)
                                .unwrap_or(false);
                        if !is_stage {
                            images.insert(from.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !seen_from {
        return Err(DockerfileProblem::Empty);
    }
    Ok(images)
}
