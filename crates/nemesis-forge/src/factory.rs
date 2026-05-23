//! Factory - creates artifacts (skills, scripts) from reflections.
//!
//! Uses LLM integration (via LLMCaller trait) to generate artifact content
//! when a provider is available. Falls back to template-based generation
//! when no LLM is configured.

use chrono::Utc;
use uuid::Uuid;

use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

use crate::reflector_llm::LLMCaller;
use crate::types::CollectedExperience;

/// Quality score result for artifact evaluation.
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    pub quality_score: f64,
    pub feedback: Vec<String>,
}

/// Factory for producing forge artifacts.
///
/// When an LLM provider is injected, `create_skill` and `create_script`
/// will call the LLM to generate high-quality content based on the
/// collected experiences. Without a provider, template-based stubs are used.
pub struct Factory {
    llm_caller: Option<Box<dyn LLMCaller>>,
}

impl Factory {
    /// Create a new factory without LLM support (template-based).
    pub fn new() -> Self {
        tracing::debug!("[Forge/Factory] Created (template-based, no LLM)");
        Self { llm_caller: None }
    }

    /// Create a new factory with LLM support.
    pub fn with_llm(caller: Box<dyn LLMCaller>) -> Self {
        tracing::info!("[Forge/Factory] Created with LLM support");
        Self {
            llm_caller: Some(caller),
        }
    }

    /// Create a skill artifact from collected experiences.
    ///
    /// When an LLM provider is available, calls the LLM to generate
    /// the skill content. Otherwise falls back to template generation.
    pub async fn create_skill(
        &self,
        name: &str,
        experiences: &[CollectedExperience],
    ) -> Artifact {
        tracing::info!(
            name = name,
            experience_count = experiences.len(),
            has_llm = self.llm_caller.is_some(),
            "[Forge/Factory] Creating skill artifact"
        );
        let tool_names: Vec<String> = experiences
            .iter()
            .map(|ce| ce.experience.tool_name.clone())
            .collect();

        let content = if let Some(ref caller) = self.llm_caller {
            tracing::debug!(name = name, "[Forge/Factory] Generating skill content via LLM");
            self.generate_skill_llm(caller.as_ref(), name, &tool_names, experiences)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(name = name, error = %e, "[Forge/Factory] LLM skill generation failed, falling back to template");
                    self.generate_skill_template(name, &tool_names, experiences)
                })
        } else {
            tracing::debug!(name = name, "[Forge/Factory] Generating skill content from template");
            self.generate_skill_template(name, &tool_names, experiences)
        };

        Artifact {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            version: "0.1.0".to_string(),
            status: ArtifactStatus::Draft,
            content,
            tool_signature: tool_names,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }
    }

    /// Create a script artifact from collected experiences.
    ///
    /// When an LLM provider is available, calls the LLM to generate
    /// the script content. Otherwise falls back to template generation.
    pub async fn create_script(
        &self,
        name: &str,
        experiences: &[CollectedExperience],
    ) -> Artifact {
        tracing::info!(
            name = name,
            experience_count = experiences.len(),
            has_llm = self.llm_caller.is_some(),
            "[Forge/Factory] Creating script artifact"
        );
        let tool_names: Vec<String> = experiences
            .iter()
            .map(|ce| ce.experience.tool_name.clone())
            .collect();

        let content = if let Some(ref caller) = self.llm_caller {
            tracing::debug!(name = name, "[Forge/Factory] Generating script content via LLM");
            self.generate_script_llm(caller.as_ref(), name, &tool_names, experiences)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(name = name, error = %e, "[Forge/Factory] LLM script generation failed, falling back to template");
                    self.generate_script_template(name, &tool_names)
                })
        } else {
            tracing::debug!(name = name, "[Forge/Factory] Generating script content from template");
            self.generate_script_template(name, &tool_names)
        };

        Artifact {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            kind: ArtifactKind::Script,
            version: "0.1.0".to_string(),
            status: ArtifactStatus::Draft,
            content,
            tool_signature: tool_names,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }
    }

    /// Evaluate an artifact and return a quality score.
    ///
    /// Checks content length, presence of required sections, and other
    /// heuristics. Returns a score in [0.0, 1.0].
    pub fn evaluate_artifact(&self, artifact: &Artifact) -> EvaluationResult {
        tracing::debug!(
            artifact_name = artifact.name,
            artifact_kind = ?artifact.kind,
            "[Forge/Factory] Evaluating artifact quality"
        );
        let mut feedback: Vec<String> = Vec::new();
        let mut score: f64 = 0.0;

        // Content length heuristic
        let len = artifact.content.len();
        if len >= 200 {
            score += 0.3;
        } else if len >= 50 {
            score += 0.15;
            feedback.push("Content is short; consider expanding the artifact.".into());
        } else {
            feedback.push("Content is very short; artifact may be incomplete.".into());
        }

        // Has tool signature
        if !artifact.tool_signature.is_empty() {
            score += 0.2;
        } else {
            feedback.push("No tool signature defined.".into());
        }

        // Contains common skill sections
        let content_lower = artifact.content.to_lowercase();
        let sections = ["description", "usage", "example"];
        let found_sections: Vec<&str> = sections
            .iter()
            .filter(|s| content_lower.contains(*s))
            .copied()
            .collect();
        score += 0.15 * found_sections.len() as f64;

        let missing: Vec<&str> = sections
            .iter()
            .filter(|s| !content_lower.contains(*s))
            .copied()
            .collect();
        if !missing.is_empty() {
            feedback.push(format!(
                "Missing recommended sections: {}",
                missing.join(", ")
            ));
        }

        // Not a draft bonus
        if artifact.status != ArtifactStatus::Draft {
            score += 0.1;
        }

        // Clamp
        score = score.clamp(0.0, 1.0);

        EvaluationResult {
            quality_score: score,
            feedback,
        }
    }

    // -----------------------------------------------------------------------
    // LLM-based content generation
    // -----------------------------------------------------------------------

    /// Generate skill content using LLM.
    async fn generate_skill_llm(
        &self,
        caller: &dyn LLMCaller,
        name: &str,
        tool_names: &[String],
        experiences: &[CollectedExperience],
    ) -> Result<String, String> {
        let success_count = experiences.iter().filter(|e| e.experience.success).count();
        let total = experiences.len();

        let tools_summary = tool_names
            .iter()
            .map(|t| format!("- {}", t))
            .collect::<Vec<_>>()
            .join("\n");

        let experience_summary = experiences
            .iter()
            .take(5)
            .map(|e| {
                format!(
                    "- Tool: {}, Input: {}, Success: {}, Output: {}",
                    e.experience.tool_name,
                    e.experience.input_summary,
                    e.experience.success,
                    e.experience.output_summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt = "You are a skill author for an AI agent system. \
            Generate a well-structured SKILL.md document that describes a reusable skill. \
            The document should include YAML frontmatter (--- delimited), a description, \
            usage instructions, examples, and notes. Respond with ONLY the skill content, \
            no additional commentary.";

        let user_prompt = format!(
            "Create a skill named '{}' based on {} tool call experiences ({} successful).\n\n\
            ## Tools Used\n{}\n\n\
            ## Sample Experiences\n{}\n\n\
            Generate a complete SKILL.md document with:\n\
            1. YAML frontmatter with name and description\n\
            2. Description of what this skill does\n\
            3. When to use this skill\n\
            4. Step-by-step usage instructions\n\
            5. Example invocation\n\
            6. Notes and caveats",
            name, total, success_count, tools_summary, experience_summary
        );

        caller.chat(system_prompt, &user_prompt, Some(2000)).await
    }

    /// Generate script content using LLM.
    async fn generate_script_llm(
        &self,
        caller: &dyn LLMCaller,
        name: &str,
        tool_names: &[String],
        experiences: &[CollectedExperience],
    ) -> Result<String, String> {
        let tools_summary = tool_names.join(", ");

        let experience_summary = experiences
            .iter()
            .take(5)
            .map(|e| {
                format!(
                    "- Tool: {}, Input: {}, Output: {}",
                    e.experience.tool_name,
                    e.experience.input_summary,
                    e.experience.output_summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt = "You are a script developer for an AI agent system. \
            Generate a well-structured bash script that automates a common task. \
            The script should be safe, well-commented, and follow best practices. \
            Respond with ONLY the script content, no additional commentary.";

        let user_prompt = format!(
            "Create a bash script named '{}' based on tool usage patterns.\n\n\
            ## Tools Referenced\n{}\n\n\
            ## Sample Experiences\n{}\n\n\
            Generate a complete bash script that:\n\
            1. Has proper shebang line and error handling (set -euo pipefail)\n\
            2. Includes helpful comments explaining each step\n\
            3. Implements the common workflow observed in the experiences\n\
            4. Has proper argument handling\n\
            5. Returns meaningful exit codes",
            name, tools_summary, experience_summary
        );

        caller.chat(system_prompt, &user_prompt, Some(2000)).await
    }

    // -----------------------------------------------------------------------
    // Template-based fallback generation
    // -----------------------------------------------------------------------

    fn generate_skill_template(
        &self,
        name: &str,
        tool_names: &[String],
        experiences: &[CollectedExperience],
    ) -> String {
        let success_count = experiences.iter().filter(|e| e.experience.success).count();
        let total = experiences.len();

        format!(
            r#"# Skill: {name}

## Description
Auto-generated skill based on {total} tool call experiences ({success_count} successful).

## Tools Used
{tools}

## Usage
Invoke this skill when the task involves the tools listed above.

## Example
```
Use {name} to accomplish the task by following the standard tool sequence.
```

## Notes
This skill was generated by the Forge self-learning framework.
Quality may vary - please review before deployment.
"#,
            name = name,
            total = total,
            success_count = success_count,
            tools = tool_names
                .iter()
                .map(|t| format!("- {}", t))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }

    fn generate_script_template(&self, name: &str, tool_names: &[String]) -> String {
        format!(
            r#"#!/usr/bin/env bash
# Script: {name}
# Auto-generated by Forge self-learning framework.

# Tools referenced: {tools}

set -euo pipefail

echo "Running {name}..."

# TODO: Add script logic based on learned patterns.
"#,
            name = name,
            tools = tool_names.join(", ")
        )
    }
}

impl Default for Factory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
