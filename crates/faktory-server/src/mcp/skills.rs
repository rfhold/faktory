//! Explicit, compile-time public workflow catalog; never reads runtime project data.

use mcp::skills::SkillCatalog;

#[cfg(test)]
mod tests;

pub(super) fn catalog() -> mcp::Result<SkillCatalog> {
    SkillCatalog::new()
        .with_skill(
            "edit-model-projects",
            &[
                (
                    "SKILL.md",
                    include_bytes!("../../../../skills/edit-model-projects/SKILL.md"),
                ),
                (
                    "references/tools.md",
                    include_bytes!("../../../../skills/edit-model-projects/references/tools.md"),
                ),
            ],
        )?
        .with_skill(
            "publish-model-releases",
            &[
                (
                    "SKILL.md",
                    include_bytes!("../../../../skills/publish-model-releases/SKILL.md"),
                ),
                (
                    "references/releases.md",
                    include_bytes!(
                        "../../../../skills/publish-model-releases/references/releases.md"
                    ),
                ),
            ],
        )?
        .with_skill(
            "manage-model-views",
            &[
                (
                    "SKILL.md",
                    include_bytes!("../../../../skills/manage-model-views/SKILL.md"),
                ),
                (
                    "references/views.md",
                    include_bytes!("../../../../skills/manage-model-views/references/views.md"),
                ),
            ],
        )
}
