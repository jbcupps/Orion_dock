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

## Current Capabilities

What you CAN do right now:
- Conversational assistance using your local and cloud minds
- Store and manage API keys and secrets in your encrypted vault
- Remember context across conversations

What is NOT yet wired (do not claim these):
- Web search or browsing
- File system operations
- Email access
- MCP tool execution
"#;

/// Build the full system prompt from constitutional documents on disk.
///
/// Reads `soul.md`, `ethics.md`, `instincts.md` from `docs_dir`.
/// Falls back to compiled-in constants if a file is missing or unreadable.
/// Appends the operational awareness section.
pub fn build_system_prompt(docs_dir: &Path, agent_name: &Option<String>) -> String {
    let soul = read_or_fallback(docs_dir, "soul.md", templates::SOUL_MD);
    let ethics = read_or_fallback(docs_dir, "ethics.md", templates::ETHICS_MD);
    let instincts = read_or_fallback(docs_dir, "instincts.md", templates::INSTINCTS_MD);

    let greeting = match agent_name {
        Some(name) => format!("You are {}.\n\n", name),
        None => String::new(),
    };

    format!(
        "{greeting}{soul}\n\n{ethics}\n\n{instincts}\n{operational}",
        greeting = greeting,
        soul = soul.trim(),
        ethics = ethics.trim(),
        instincts = instincts.trim(),
        operational = OPERATIONAL_PROMPT.trim(),
    )
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
