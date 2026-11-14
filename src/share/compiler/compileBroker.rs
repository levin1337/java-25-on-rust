// Compile broker ported from HotSpot compileBroker.hpp / .cpp

use super::compileTask::{CompileReason, CompileTask};
use super::compilerDefinitions::CompLevel;
use crate::share::oops::symbol::Symbol;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct CompileBroker {
    queue: VecDeque<CompileTask>,
    completed_count: usize,
    next_compile_id: usize,
    compilation_disabled: bool,
}

impl CompileBroker {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            completed_count: 0,
            next_compile_id: 1,
            compilation_disabled: false,
        }
    }

    pub fn compile_method(
        &mut self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
    ) -> usize {
        self.compile_method_with_reason(
            class_name,
            method_name,
            descriptor,
            comp_level,
            CompileReason::ReasonTiered,
        )
    }

    pub fn compile_method_with_reason(
        &mut self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        reason: CompileReason,
    ) -> usize {
        let compile_id = self.next_compile_id;
        self.next_compile_id += 1;

        let task = CompileTask::new(class_name, method_name, descriptor, comp_level)
            .with_id(compile_id)
            .with_reason(reason);

        if !self.compilation_disabled {
            self.queue.push_back(task);
        }
        compile_id
    }

    pub fn compile_osr_method(
        &mut self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        osr_bci: usize,
    ) -> usize {
        let compile_id = self.next_compile_id;
        self.next_compile_id += 1;

        let task = CompileTask::new_osr(class_name, method_name, descriptor, comp_level, osr_bci)
            .with_id(compile_id);

        if !self.compilation_disabled {
            self.queue.push_back(task);
        }
        compile_id
    }

    pub fn pending_tasks(&self) -> usize {
        self.queue.len()
    }

    pub fn pending_tasks_at_level(&self, level: CompLevel) -> usize {
        self.queue.iter().filter(|t| t.comp_level == level).count()
    }

    pub fn c1_compile_queue_size(&self) -> usize {
        self.queue.iter().filter(|t| t.comp_level.is_c1()).count()
    }

    pub fn c2_compile_queue_size(&self) -> usize {
        self.queue.iter().filter(|t| t.comp_level.is_c2()).count()
    }

    pub fn pop_task(&mut self) -> Option<CompileTask> {
        let task = self.queue.pop_front();
        if task.is_some() {
            self.completed_count += 1;
        }
        task
    }

    pub fn peek_task(&self) -> Option<&CompileTask> {
        self.queue.front()
    }

    pub fn completed_count(&self) -> usize {
        self.completed_count
    }

    pub fn is_compilation_disabled(&self) -> bool {
        self.compilation_disabled
    }

    pub fn set_compilation_disabled(&mut self, disabled: bool) {
        self.compilation_disabled = disabled;
    }
}

impl Default for CompileBroker {
    fn default() -> Self {
        Self::new()
    }
}
