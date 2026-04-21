use serde::Serialize;

use super::TagCategory;

/// Built-in tag names and their categories. Keep in sync with
/// `src/utils/tagColors.ts` on the frontend.
///
/// `keep` is a reserved review-state tag: users apply it bulk from the
/// Review view to mark segments they've decided to preserve, and the
/// Review view's default filter hides anything with this tag.
pub const BUILTIN_TAGS: &[(&str, TagCategory)] = &[
    ("event", TagCategory::Event),
    ("stationary", TagCategory::Motion),
    ("silent", TagCategory::Audio),
    ("no_audio", TagCategory::Audio),
    ("keep", TagCategory::User),
];

/// A developer-curated tag that the user can apply to a segment
/// manually (from the player tag bar or the Review view's bulk
/// dropdown). Distinct from scan-emitted tags like `stationary` or
/// `silent` — those are produced by the analysis pipeline and
/// applying them by hand would be misleading.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserApplicableTag {
    pub name: &'static str,
    pub category: TagCategory,
    pub display_name: &'static str,
    pub description: &'static str,
}

/// The full set of tags the UI exposes as user-applicable. Order here
/// determines left-to-right order of pills on the player tag bar and
/// menu-item order in the Review view's dropdowns.
pub const USER_APPLICABLE_TAGS: &[UserApplicableTag] = &[
    UserApplicableTag {
        name: "keep",
        category: TagCategory::User,
        display_name: "Keep",
        description: "Reviewed and worth keeping. Hidden from the Review view's default filter so repeated review sessions surface only unreviewed content.",
    },
];

/// Look up a built-in tag's category by name. Returns `None` for
/// user-defined free-form tags.
pub fn builtin_category(name: &str) -> Option<TagCategory> {
    BUILTIN_TAGS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, cat)| *cat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_is_reserved_as_user_category() {
        assert_eq!(builtin_category("keep"), Some(TagCategory::User));
    }

    #[test]
    fn unknown_tag_returns_none() {
        assert_eq!(builtin_category("arbitrary_user_tag"), None);
    }

    #[test]
    fn user_applicable_tags_excludes_system_tags() {
        // These are emitted by scans; exposing them in the user-apply
        // UI would let the user fake scan output, which defeats the
        // source=system|user distinction.
        let names: Vec<&str> = USER_APPLICABLE_TAGS.iter().map(|t| t.name).collect();
        assert!(!names.contains(&"stationary"));
        assert!(!names.contains(&"silent"));
        assert!(!names.contains(&"no_audio"));
        assert!(!names.contains(&"event"));
    }

    #[test]
    fn user_applicable_tags_are_all_in_builtin_tags() {
        // Every user-applicable tag must also exist in BUILTIN_TAGS so
        // the category lookup at tag-insert time succeeds.
        for t in USER_APPLICABLE_TAGS {
            assert_eq!(
                builtin_category(t.name),
                Some(t.category),
                "user-applicable tag {} missing from BUILTIN_TAGS or has mismatched category",
                t.name,
            );
        }
    }
}
