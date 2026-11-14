/*
 * Port of OpenJDK 25 HotSpot CompilerOracle (share/compiler/compilerOracle.hpp / compilerOracle.cpp)
 *
 * Implements JIT compilation command processing: -XX:CompileCommand=<cmd>,<class>.<method>
 * allowing granular per-method control over inlining, exclusion, and logging.
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleCommand {
    Break,
    Print,
    Exclude,
    Inline,
    DontInline,
    CompileOnly,
    Log,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleRule {
    pub command: OracleCommand,
    pub class_pattern: String,
    pub method_pattern: String,
}

impl OracleRule {
    pub fn matches(&self, class_name: &str, method_name: &str) -> bool {
        Self::pattern_matches(&self.class_pattern, class_name)
            && Self::pattern_matches(&self.method_pattern, method_name)
    }

    fn pattern_matches(pattern: &str, target: &str) -> bool {
        if pattern == "*" || pattern.is_empty() {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return target.starts_with(prefix);
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            return target.ends_with(suffix);
        }
        pattern == target
    }
}

pub struct CompilerOracle {
    rules: Vec<OracleRule>,
}

impl CompilerOracle {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn register_command(
        &mut self,
        cmd: OracleCommand,
        class_pattern: &str,
        method_pattern: &str,
    ) {
        self.rules.push(OracleRule {
            command: cmd,
            class_pattern: class_pattern.to_string(),
            method_pattern: method_pattern.to_string(),
        });
    }

    /// Parses command string like "dontinline,java/lang/String.charAt" or "exclude,com/app/*.*"
    pub fn parse_compile_command(&mut self, spec: &str) -> Result<(), &'static str> {
        let (cmd_str, target) = spec
            .split_once(',')
            .ok_or("Expected comma in CompileCommand")?;

        let command = match cmd_str.trim().to_lowercase().as_str() {
            "break" => OracleCommand::Break,
            "print" => OracleCommand::Print,
            "exclude" => OracleCommand::Exclude,
            "inline" => OracleCommand::Inline,
            "dontinline" => OracleCommand::DontInline,
            "compileonly" => OracleCommand::CompileOnly,
            "log" => OracleCommand::Log,
            _ => return Err("Unknown OracleCommand"),
        };

        let (class_pat, method_pat) = if let Some((c, m)) = target.trim().split_once('.') {
            (c, m)
        } else {
            (target.trim(), "*")
        };

        self.register_command(command, class_pat, method_pat);
        Ok(())
    }

    pub fn should_inline(&self, class_name: &str, method_name: &str) -> bool {
        if self.should_not_inline(class_name, method_name) {
            return false;
        }
        self.rules
            .iter()
            .any(|r| r.command == OracleCommand::Inline && r.matches(class_name, method_name))
    }

    pub fn should_not_inline(&self, class_name: &str, method_name: &str) -> bool {
        self.rules
            .iter()
            .any(|r| r.command == OracleCommand::DontInline && r.matches(class_name, method_name))
    }

    pub fn should_exclude(&self, class_name: &str, method_name: &str) -> bool {
        let has_compile_only = self
            .rules
            .iter()
            .any(|r| r.command == OracleCommand::CompileOnly);
        if has_compile_only {
            let matched_compile_only = self.rules.iter().any(|r| {
                r.command == OracleCommand::CompileOnly && r.matches(class_name, method_name)
            });
            if !matched_compile_only {
                return true;
            }
        }

        self.rules
            .iter()
            .any(|r| r.command == OracleCommand::Exclude && r.matches(class_name, method_name))
    }

    pub fn should_print(&self, class_name: &str, method_name: &str) -> bool {
        self.rules
            .iter()
            .any(|r| r.command == OracleCommand::Print && r.matches(class_name, method_name))
    }
}

impl Default for CompilerOracle {
    fn default() -> Self {
        Self::new()
    }
}
