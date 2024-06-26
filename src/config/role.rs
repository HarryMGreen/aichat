use super::Input;
use crate::{
    client::{Message, MessageContent, MessageRole},
    utils::{detect_os, detect_shell},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const TEMP_ROLE: &str = "%%";
pub const SHELL_ROLE: &str = "%shell%";
pub const EXPLAIN_SHELL_ROLE: &str = "%explain-shell%";
pub const CODE_ROLE: &str = "%code%";

pub const INPUT_PLACEHOLDER: &str = "__INPUT__";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Role {
    pub name: String,
    pub prompt: String,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
}

impl Role {
    pub fn temp(prompt: &str) -> Self {
        Self {
            name: TEMP_ROLE.into(),
            prompt: prompt.into(),
            temperature: None,
            top_p: None,
        }
    }

    pub fn builtin() -> Vec<Role> {
        [
            (SHELL_ROLE, shell_prompt()),
            (
                EXPLAIN_SHELL_ROLE,
                r#"Provide a terse, single sentence description of the given shell command.
Describe each argument and option of the command.
Provide short responses in about 80 words.
APPLY MARKDOWN formatting when possible."#
                    .into(),
            ),
            (
                CODE_ROLE,
                r#"Provide only code without comments or explanations.
### INPUT:
async sleep in js
### OUTPUT:
```javascript
async function timeout(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}
```
"#
                .into(),
            ),
        ]
        .into_iter()
        .map(|(name, prompt)| Self {
            name: name.into(),
            prompt,
            temperature: None,
            top_p: None,
        })
        .collect()
    }

    pub fn export(&self) -> Result<String> {
        let output = serde_yaml::to_string(&self)
            .with_context(|| format!("Unable to show info about role {}", &self.name))?;
        Ok(output.trim_end().to_string())
    }

    pub fn embedded(&self) -> bool {
        self.prompt.contains(INPUT_PLACEHOLDER)
    }

    pub fn set_temperature(&mut self, value: Option<f64>) {
        self.temperature = value;
    }

    pub fn set_top_p(&mut self, value: Option<f64>) {
        self.top_p = value;
    }

    pub fn complete_prompt_args(&mut self, name: &str) {
        self.name = name.to_string();
        self.prompt = complete_prompt_args(&self.prompt, &self.name);
    }

    pub fn match_name(&self, name: &str) -> bool {
        if self.name.contains(':') {
            let role_name_parts: Vec<&str> = self.name.split(':').collect();
            let name_parts: Vec<&str> = name.split(':').collect();
            role_name_parts[0] == name_parts[0] && role_name_parts.len() == name_parts.len()
        } else {
            self.name == name
        }
    }

    pub fn echo_messages(&self, input: &Input) -> String {
        let input_markdown = input.render();
        if self.embedded() {
            self.prompt.replace(INPUT_PLACEHOLDER, &input_markdown)
        } else {
            format!("{}\n\n{}", self.prompt, input.render())
        }
    }

    pub fn build_messages(&self, input: &Input) -> Vec<Message> {
        let mut content = input.to_message_content();

        if self.embedded() {
            content.merge_prompt(|v: &str| self.prompt.replace(INPUT_PLACEHOLDER, v));
            vec![Message {
                role: MessageRole::User,
                content,
            }]
        } else {
            let mut messages = vec![];
            let (system, cases) = parse_structure_prompt(&self.prompt);
            if !system.is_empty() {
                messages.push(Message {
                    role: MessageRole::System,
                    content: MessageContent::Text(system.to_string()),
                })
            }
            if !cases.is_empty() {
                messages.extend(cases.into_iter().flat_map(|(i, o)| {
                    vec![
                        Message {
                            role: MessageRole::User,
                            content: MessageContent::Text(i.to_string()),
                        },
                        Message {
                            role: MessageRole::Assistant,
                            content: MessageContent::Text(o.to_string()),
                        },
                    ]
                }));
            }
            messages.push(Message {
                role: MessageRole::User,
                content,
            });
            messages
        }
    }
}

fn complete_prompt_args(prompt: &str, name: &str) -> String {
    let mut prompt = prompt.trim().to_string();
    for (i, arg) in name.split(':').skip(1).enumerate() {
        prompt = prompt.replace(&format!("__ARG{}__", i + 1), arg);
    }
    prompt
}

fn parse_structure_prompt(prompt: &str) -> (&str, Vec<(&str, &str)>) {
    let mut text = prompt;
    let mut search_input = true;
    let mut system = None;
    let mut parts = vec![];
    loop {
        let search = if search_input {
            "### INPUT:"
        } else {
            "### OUTPUT:"
        };
        match text.find(search) {
            Some(idx) => {
                if system.is_none() {
                    system = Some(&text[..idx])
                } else {
                    parts.push(&text[..idx])
                }
                search_input = !search_input;
                text = &text[(idx + search.len())..];
            }
            None => {
                if !text.is_empty() {
                    if system.is_none() {
                        system = Some(text)
                    } else {
                        parts.push(text)
                    }
                }
                break;
            }
        }
    }
    let parts_len = parts.len();
    if parts_len > 0 && parts_len % 2 == 0 {
        let cases: Vec<(&str, &str)> = parts
            .iter()
            .step_by(2)
            .zip(parts.iter().skip(1).step_by(2))
            .map(|(i, o)| (i.trim(), o.trim()))
            .collect();
        let system = system.map(|v| v.trim()).unwrap_or_default();
        return (system, cases);
    }

    (prompt, vec![])
}

fn shell_prompt() -> String {
    let os = detect_os();
    let (detected_shell, _, _) = detect_shell();
    let (shell, use_semicolon) = match (detected_shell.as_str(), os.as_str()) {
        // GPT doesn’t know much about nushell
        ("nushell", "windows") => ("cmd", true),
        ("nushell", _) => ("bash", true),
        ("powershell", _) => ("powershell", true),
        ("pwsh", _) => ("powershell", false),
        _ => (detected_shell.as_str(), false),
    };
    let combine = if use_semicolon {
        "\nIf multiple steps required try to combine them together using ';'.\nIf it already combined with '&&' try to replace it with ';'.".to_string()
    } else {
        "\nIf multiple steps required try to combine them together using '&&'.".to_string()
    };
    format!(
        r#"Provide only {shell} commands for {os} without any description.
Ensure the output is a valid {shell} command. {combine}
If there is a lack of details, provide most logical solution.
Output plain text only, without any markdown formatting."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_prompt_name() {
        assert_eq!(
            complete_prompt_args("convert __ARG1__", "convert:foo"),
            "convert foo"
        );
        assert_eq!(
            complete_prompt_args("convert __ARG1__ to __ARG2__", "convert:foo:bar"),
            "convert foo to bar"
        );
    }

    #[test]
    fn test_parse_structure_prompt1() {
        let prompt = r#"
System message
### INPUT:
Input 1
### OUTPUT:
Output 1
"#;
        assert_eq!(
            parse_structure_prompt(prompt),
            ("System message", vec![("Input 1", "Output 1")])
        );
    }

    #[test]
    fn test_parse_structure_prompt2() {
        let prompt = r#"
### INPUT:
Input 1
### OUTPUT:
Output 1
"#;
        assert_eq!(
            parse_structure_prompt(prompt),
            ("", vec![("Input 1", "Output 1")])
        );
    }

    #[test]
    fn test_parse_structure_prompt3() {
        let prompt = r#"
System message
### INPUT:
Input 1
"#;
        assert_eq!(parse_structure_prompt(prompt), (prompt, vec![]));
    }
}
