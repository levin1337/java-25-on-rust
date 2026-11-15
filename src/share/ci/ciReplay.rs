/*
 * Port of OpenJDK 25 HotSpot ciReplay (share/ci/ciReplay.hpp/.cpp)
 *
 * Compile-replay support for `-XX:+ReplayCompiles`: the VM records
 * the sequence of compile tasks (and their inlining decisions) to a
 * replay file; on replay the same tasks are re-driven deterministically.
 * This module implements the ci-side data model: parsing and emitting
 * replay commands and answering `should_inline`-style queries for the
 * compiler, layered on top of `CompilerOracle` rules.
 *
 * Replay file format (subset of HotSpot's):
 *   compile <id> <holder>.<name> <signature>
 *   ciMethod <id> <holder> <name> <signature> <bci> <opcode> ...
 *   inline <id> <bci> <callee holder>.<name> <sig> ...
 *   unknown <text>
 */

use crate::share::compiler::compilerOracle::{CompilerOracle, InlineDecision};
use std::io::{self, BufRead, Write};

/// One recorded compile task (`ciReplay::compile` analogue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCompile {
    /// Task id (HotSpot uses the compilation id).
    pub id: u64,
    /// Holder internal name (`java/lang/String`).
    pub holder: String,
    /// Method name (`charAt`).
    pub name: String,
    /// Method descriptor (`(I)C`).
    pub signature: String,
}

impl ReplayCompile {
    /// `holder.name signature` query key.
    pub fn key(&self) -> String {
        format!("{}.{} {}", self.holder, self.name, self.signature)
    }

    /// Parse a `compile` line's method part: `holder.name(sig)`.
    pub fn parse_method(spec: &str) -> Option<(String, String, String)> {
        let spec = spec.trim();
        let (body, sig) = if spec.ends_with(')') {
            let open = spec.rfind('(')?;
            (&spec[..open], spec[open..].to_string())
        } else {
            (spec, String::new())
        };
        let dot = body.rfind('.')?;
        let (holder, name) = body.split_at(dot);
        Some((
            holder.to_string(),
            name[1..].to_string(),
            sig,
        ))
    }
}

/// One recorded bytecode/inlining directive inside a compile task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayCommand {
    /// `compile <id> <method>` — begin a recorded compilation.
    Compile(ReplayCompile),
    /// `ciMethod ...` — method metadata snapshot (kept verbatim for
    /// tooling; the compiler re-derives metadata from the loader).
    CiMethod { id: u64, text: String },
    /// `inline <compile_id> <bci> <callee>` — the recorded inlining
    /// decision for a call site.
    Inline {
        compile_id: u64,
        bci: usize,
        callee: ReplayCompile,
    },
    /// Unrecognized line preserved verbatim.
    Unknown(String),
}

/// `ciReplay`: the recorded (or loaded) sequence of compile commands.
pub struct CiReplay {
    commands: Vec<ReplayCommand>,
    /// Whether replay mode is active (`-XX:+ReplayCompiles`).
    enabled: bool,
}

impl CiReplay {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            enabled: false,
        }
    }

    /// `-XX:+ReplayCompiles`.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// `-XX:-ReplayCompiles`.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// All recorded commands in order.
    pub fn commands(&self) -> &[ReplayCommand] {
        &self.commands
    }

    /// Record a compile task (`ciReplay::record` analogue).
    pub fn record_compile(&mut self, compile: ReplayCompile) {
        self.commands.push(ReplayCommand::Compile(compile));
    }

    /// Record a generic replay line (ciMethod snapshots, inline
    /// directives parsed separately).
    pub fn record_line(&mut self, line: &str) {
        self.commands.push(ReplayCommand::Unknown(line.to_string()));
    }

    /// Parse one replay-file line into a `ReplayCommand`.
    /// Returns `None` for blank/comment lines.
    pub fn parse_line(line: &str) -> Option<ReplayCommand> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut it = line.splitn(2, char::is_whitespace);
        let tag = it.next()?;
        let rest = it.next().unwrap_or("").trim();
        match tag {
            "compile" => {
                let mut parts = rest.splitn(2, char::is_whitespace);
                let id: u64 = parts.next()?.parse().ok()?;
                let (holder, name, signature) = ReplayCompile::parse_method(parts.next()?)?;
                Some(ReplayCommand::Compile(ReplayCompile {
                    id,
                    holder,
                    name,
                    signature,
                }))
            }
            "ciMethod" => Some(ReplayCommand::CiMethod {
                id: rest.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0),
                text: rest.to_string(),
            }),
            "inline" => {
                let mut parts = rest.splitn(3, char::is_whitespace);
                let compile_id: u64 = parts.next()?.parse().ok()?;
                let bci: usize = parts.next()?.parse().ok()?;
                let (holder, name, signature) = ReplayCompile::parse_method(parts.next()?)?;
                Some(ReplayCommand::Inline {
                    compile_id,
                    bci,
                    callee: ReplayCompile {
                        id: 0,
                        holder,
                        name,
                        signature,
                    },
                })
            }
            _ => Some(ReplayCommand::Unknown(line.to_string())),
        }
    }

    /// Load a replay file (`ciReplay::load` analogue).
    pub fn load<R: BufRead>(&mut self, reader: R) -> io::Result<usize> {
        let mut count = 0usize;
        for line in reader.lines() {
            let line = line?;
            if let Some(cmd) = Self::parse_line(&line) {
                self.commands.push(cmd);
                count += 1;
            }
        }
        Ok(count)
    }

    /// Write the replay file (`ciReplay::dump` analogue).
    pub fn write_to<W: Write>(&self, mut w: W) -> io::Result<()> {
        for cmd in &self.commands {
            match cmd {
                ReplayCommand::Compile(c) => writeln!(
                    w,
                    "compile {} {}.{} {}",
                    c.id, c.holder, c.name, c.signature
                )?,
                ReplayCommand::CiMethod { id, text } => {
                    writeln!(w, "ciMethod {} {}", id, text)?
                }
                ReplayCommand::Inline {
                    compile_id,
                    bci,
                    callee,
                } => writeln!(
                    w,
                    "inline {} {} {}.{} {}",
                    compile_id, bci, callee.holder, callee.name, callee.signature
                )?,
                ReplayCommand::Unknown(text) => writeln!(w, "{}", text)?,
            }
        }
        Ok(())
    }

    /// Recorded compile tasks in order.
    pub fn compiles(&self) -> impl Iterator<Item = &ReplayCompile> {
        self.commands.iter().filter_map(|c| match c {
            ReplayCommand::Compile(c) => Some(c),
            _ => None,
        })
    }

    /// Recorded inline decision for `(compile_id, bci)`, if any.
    pub fn inline_at(&self, compile_id: u64, bci: usize) -> Option<&ReplayCompile> {
        self.commands.iter().find_map(|c| match c {
            ReplayCommand::Inline {
                compile_id: id,
                bci: b,
                callee,
            } if *id == compile_id && *b == bci => Some(callee),
            _ => None,
        })
    }

    /// `ciReplay::should_inline` analogue: consults the replayed inline
    /// stream first (when replay is enabled), then falls back to the
    /// `CompilerOracle` global rules (`-XX:CompileCommand=inline`).
    pub fn should_inline(
        &self,
        oracle: &CompilerOracle,
        compile_id: u64,
        bci: usize,
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> InlineDecision {
        if self.enabled {
            if let Some(callee) = self.inline_at(compile_id, bci) {
                let _ = callee;
                return InlineDecision::Inline;
            }
            // Replay mode: only recorded inlines are honored.
            return InlineDecision::DontInline;
        }
        oracle.inline_decision(class_name, method_name, signature)
    }
}

impl Default for CiReplay {
    fn default() -> Self {
        Self::new()
    }
}
