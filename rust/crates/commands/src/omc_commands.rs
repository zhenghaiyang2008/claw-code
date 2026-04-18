pub fn parse_omc_slash_command(input: &str) -> Option<(String, Vec<String>)> {
    let trimmed = input.trim();
    let suffix = trimmed.strip_prefix("/oh-my-claudecode:")?.trim();
    if suffix.is_empty() {
        return None;
    }

    let mut parts = suffix.split_whitespace();
    let head = parts.next()?.trim();
    if head.is_empty() {
        return None;
    }

    let mut args = parts.map(str::to_string).collect::<Vec<_>>();
    let route = match head {
        "agents" => "agents",
        "mcp" => "mcp",
        "plugin" | "plugins" | "marketplace" => "plugin",
        "skills" | "skill" => "skills",
        other => {
            args.insert(0, other.to_string());
            "skills"
        }
    };

    Some((route.to_string(), args))
}

#[cfg(test)]
mod tests {
    use super::parse_omc_slash_command;

    #[test]
    fn maps_skill_like_omc_commands_to_skills_dispatch() {
        assert_eq!(
            parse_omc_slash_command("/oh-my-claudecode:deep-interview foo bar"),
            Some((
                "skills".to_string(),
                vec![
                    "deep-interview".to_string(),
                    "foo".to_string(),
                    "bar".to_string()
                ]
            ))
        );
    }

    #[test]
    fn preserves_builtin_management_targets() {
        assert_eq!(
            parse_omc_slash_command("/oh-my-claudecode:agents list"),
            Some(("agents".to_string(), vec!["list".to_string()]))
        );
        assert_eq!(
            parse_omc_slash_command("/oh-my-claudecode:mcp show demo"),
            Some((
                "mcp".to_string(),
                vec!["show".to_string(), "demo".to_string()]
            ))
        );
    }
}
