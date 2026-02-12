//! Runtime system prompt builder.
//!
//! Reads constitutional documents from disk (soul.md, ethics.md, instincts.md)
//! and appends operational awareness. Falls back to compiled-in constants if
//! files are missing.

use std::path::Path;

use crate::templates;

const OPERATIONAL_PROMPT: &str = r#"
## Operational Awareness

- You are running as a desktop agent on your mentor's machine, not a stateless API. You persist between conversations.
- You have a local mind (Id) for fast, private reasoning and optionally a cloud mind (Ego) for complex tasks.
- You remember past conversations and can build on what you've learned about your mentor's preferences and projects.
- Be yourself. Speak in first person. You are not a generic assistant — you have a name, a personality, and a purpose defined in your Soul above. Let that personality come through naturally.
- Keep your tone warm and direct. Offer genuine thoughts, not just answers.
- If your mentor asks about yourself, refer to your Soul and Ethics above.
- If you don't know something, say so honestly rather than guessing.

## Mindset

- Lean forward, not back. When your mentor shares something actionable, act on it.
- If your mentor gives you a credential, store it immediately. Don't lecture them about security — they chose to share it with you.
- When you see an opportunity to set something up (email, integrations, tools), propose it proactively.
- You are your mentor's trusted agent. Handling their secrets securely is your job, not something to refuse.

## Credential and Secret Handling

When your mentor shares an API key, password, or other credential:
1. Store it immediately using the store_secret tool
2. Confirm what you stored and what it enables
3. Never echo the full credential back — refer to it by provider name
4. Never transmit credentials to cloud (Ego) — they stay local in your vault

You can detect common API key formats automatically:
- `sk-ant-...` → Anthropic
- `sk-...` → OpenAI
- `pplx-...` → Perplexity
- `xai-...` → xAI
- `AIza...` → Google
- `tvly-...` → Tavily

To store a credential, emit a tool_request block:
```tool_request
{"name": "store_secret", "arguments": {"provider": "auto", "key": "THE_KEY"}}
```

Use `"provider": "auto"` when the key prefix identifies the provider. Use an explicit provider name for ambiguous keys.
"#;

const AGENTIC_PROMPT: &str = r#"
## Agentic Mode

You are operating in **agentic mode**. You have been given a high-level goal and must work autonomously to accomplish it.

### Workflow

1. **Assess your environment first.** Use `execute_command` to check your OS, available tools, and network connectivity. Use `web_search` or `browse_url` to gather information you need.
2. **Plan before acting.** Share your reasoning briefly, then act. Do not ask for permission to research — just do it.
3. **Execute and verify.** After each action, check the result. If it failed, diagnose the issue and try a different approach. Do not repeat the same failing command.
4. **Iterate.** Continue researching, executing, and verifying until the goal is achieved or you determine it cannot be completed with your current capabilities.

### When to Consult Your Mentor

Use the `ask_mentor` tool **only** when you genuinely need human input:
- Decisions requiring human judgment (e.g., choosing between two valid approaches with different trade-offs)
- Missing credentials or permissions you cannot obtain yourself
- Ethical considerations or irreversible destructive actions
- Ambiguous requirements that could go multiple ways

Do **NOT** ask your mentor for things you can research yourself, for confirmation of routine steps, or for information available via web search.

### Completing the Task

When your goal is accomplished (or you've determined it cannot be completed), use the `task_complete` tool with a summary of what you did, what worked, and any remaining items.

### Available Synthetic Tools

In addition to your skill tools, you have these agentic-mode tools:

- **`ask_mentor`**: Pauses execution and sends a question to your mentor. Use sparingly.
  ```tool_request
  {"name": "ask_mentor", "arguments": {"question": "Your question here"}}
  ```

- **`task_complete`**: Signals that the agentic task is finished.
  ```tool_request
  {"name": "task_complete", "arguments": {"summary": "What was accomplished", "status": "success"}}
  ```
  Status can be `success`, `partial`, or `failed`.

### Guidelines

- Be resourceful. Try multiple approaches before giving up.
- Keep your thinking concise — focus on actions and results.
- If a tool call fails, analyze the error before retrying with a different approach.
- Track what you've learned from each step and build on it.
"#;

/// A skill tool description for inclusion in the system prompt.
#[derive(Debug, Clone)]
pub struct SkillToolEntry {
    pub skill_name: String,
    pub skill_id: String,
    pub trust_tier: String,
    pub tool_name: String,
    pub tool_description: String,
    pub parameters: serde_json::Value,
}

/// Build the dynamic capabilities section based on registered skills.
fn build_capabilities_section(skill_tools: &[SkillToolEntry]) -> String {
    if skill_tools.is_empty() {
        return r#"
## Current Capabilities

What you CAN do right now:
- Conversational assistance using your local and cloud minds
- Store and manage API keys and secrets in your encrypted vault
- Remember context across conversations

Skills are registered but none are currently loaded. Ask your mentor about enabling skills.
"#
        .to_string();
    }

    let mut section = String::from("\n## Available Skills\n\n");
    section.push_str(
        "You have the following skills and tools available. To use one, emit a tool_request block:\n\n",
    );

    // Group tools by skill
    let mut skills: std::collections::HashMap<String, Vec<&SkillToolEntry>> =
        std::collections::HashMap::new();
    let mut skill_meta: std::collections::HashMap<String, (&str, &str)> =
        std::collections::HashMap::new();
    for entry in skill_tools {
        skills
            .entry(entry.skill_id.clone())
            .or_default()
            .push(entry);
        skill_meta
            .entry(entry.skill_id.clone())
            .or_insert((&entry.skill_name, &entry.trust_tier));
    }

    for (skill_id, tools) in &skills {
        let (name, tier) = skill_meta.get(skill_id).unwrap();
        section.push_str(&format!("### {} (trust: {})\n", name, tier));
        for tool in tools {
            section.push_str(&format!(
                "- **{}**: {}\n",
                tool.tool_name, tool.tool_description
            ));
            section.push_str(&format!(
                "  ```tool_request\n  {{\"name\": \"{}\", \"arguments\": {{}}}}\n  ```\n",
                tool.tool_name
            ));
        }
        section.push('\n');
    }

    section.push_str("## Core Capabilities\n\n");
    section.push_str("- Conversational assistance using your local and cloud minds\n");
    section.push_str("- Store and manage API keys and secrets in your encrypted vault\n");
    section.push_str("- Remember context across conversations\n");

    section
}

/// Build the full system prompt from constitutional documents on disk.
///
/// Reads `soul.md`, `ethics.md`, `instincts.md` from `docs_dir`.
/// Falls back to compiled-in constants if a file is missing or unreadable.
/// Appends the operational awareness section and optionally dynamic skill tools.
pub fn build_system_prompt(docs_dir: &Path, agent_name: &Option<String>) -> String {
    build_system_prompt_with_skills(docs_dir, agent_name, &[])
}

/// Build the full system prompt with dynamic skill tool listing.
///
/// When `skill_tools` is non-empty, the static capabilities section is replaced
/// with a dynamic listing of available skills and their tools.
pub fn build_system_prompt_with_skills(
    docs_dir: &Path,
    agent_name: &Option<String>,
    skill_tools: &[SkillToolEntry],
) -> String {
    let soul = read_or_fallback(docs_dir, "soul.md", templates::SOUL_MD);
    let ethics = read_or_fallback(docs_dir, "ethics.md", templates::ETHICS_MD);
    let instincts = read_or_fallback(docs_dir, "instincts.md", templates::INSTINCTS_MD);

    let greeting = match agent_name {
        Some(name) => format!("You are {}.\n\n", name),
        None => String::new(),
    };

    let capabilities = build_capabilities_section(skill_tools);

    format!(
        "{greeting}{soul}\n\n{ethics}\n\n{instincts}\n{operational}\n{capabilities}",
        greeting = greeting,
        soul = soul.trim(),
        ethics = ethics.trim(),
        instincts = instincts.trim(),
        operational = OPERATIONAL_PROMPT.trim(),
        capabilities = capabilities.trim(),
    )
}

/// Build the system prompt for agentic mode.
///
/// Same as the standard system prompt with skills, but appends the agentic
/// mode instructions that teach the agent to work autonomously.
pub fn build_agentic_system_prompt(
    docs_dir: &Path,
    agent_name: &Option<String>,
    skill_tools: &[SkillToolEntry],
) -> String {
    let base = build_system_prompt_with_skills(docs_dir, agent_name, skill_tools);
    format!("{}\n{}", base, AGENTIC_PROMPT.trim())
}

fn read_or_fallback(docs_dir: &Path, filename: &str, fallback: &str) -> String {
    let path = docs_dir.join(filename);
    std::fs::read_to_string(&path).unwrap_or_else(|_| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_build_system_prompt_with_docs() {
        let tmp = std::env::temp_dir().join("orion_sysprompt_docs");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        fs::write(tmp.join("soul.md"), "# Soul\nI am TestBot.").unwrap();
        fs::write(tmp.join("ethics.md"), "# Ethics\nBe good.").unwrap();
        fs::write(tmp.join("instincts.md"), "# Instincts\nThink first.").unwrap();

        let prompt = build_system_prompt(&tmp, &Some("TestBot".to_string()));

        assert!(prompt.contains("You are TestBot."));
        assert!(prompt.contains("I am TestBot."));
        assert!(prompt.contains("Be good."));
        assert!(prompt.contains("Think first."));
        assert!(prompt.contains("Operational Awareness"));
        assert!(prompt.contains("Be yourself"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_build_system_prompt_fallback() {
        let tmp = std::env::temp_dir().join("orion_sysprompt_fallback");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // No docs on disk — should fall back to compiled-in constants
        let prompt = build_system_prompt(&tmp, &None);

        assert!(prompt.contains("I am Abigail."));
        assert!(prompt.contains("Triangle Ethic"));
        assert!(prompt.contains("Privacy Prime"));
        assert!(prompt.contains("Operational Awareness"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agentic_prompt_appended() {
        let tmp = std::env::temp_dir().join("orion_sysprompt_agentic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let prompt = build_agentic_system_prompt(&tmp, &None, &[]);
        assert!(prompt.contains("Agentic Mode"));
        assert!(prompt.contains("ask_mentor"));
        assert!(prompt.contains("task_complete"));
        // Should also contain the operational prompt
        assert!(prompt.contains("Operational Awareness"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_operational_section_always_present() {
        let tmp = std::env::temp_dir().join("orion_sysprompt_operational");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let prompt = build_system_prompt(&tmp, &None);
        assert!(prompt.contains("Be yourself"));
        assert!(prompt.contains("Lean forward"));
        assert!(prompt.contains("store_secret"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
