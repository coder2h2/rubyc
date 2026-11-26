use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::*;

#[derive(Clone, Debug)]
enum VarLoc {
    Stack(i32),      // [rbp - offset]
    Env(usize, usize), // (depth, slot_index)
}

#[derive(Clone)]
struct ScopeInfo {
    locals: HashMap<String, VarLoc>,
    captured_names: Vec<String>,
    has_env: bool,
    parent_env_slot: Option<i32>, // [rbp - offset] holding this scope's env ptr
}

struct FnState {
    next_local_offset: i32,
    temp_offset: i32,
    max_temp_offset: i32,
    epilogue_label: String,
    loop_stack: Vec<(String, String)>, // (continue_label, break_label)
    scopes: Vec<ScopeInfo>,
    self_slot: i32,
    argc_slot: i32,
    argv_slot: i32,
    block_slot: i32,
    defining_class_slot: Option<i32>,
    current_method_name: Option<String>,
    is_block: bool,
}

impl FnState {
    fn alloc_stack(&mut self, bytes: i32) -> i32 {
        self.next_local_offset += bytes;
        if self.next_local_offset > self.temp_offset {
            self.temp_offset = self.next_local_offset;
        }
        if self.temp_offset > self.max_temp_offset {
            self.max_temp_offset = self.temp_offset;
        }
        self.next_local_offset
    }

    fn alloc_temp(&mut self) -> i32 {
        self.temp_offset += 8;
        if self.temp_offset > self.max_temp_offset {
            self.max_temp_offset = self.temp_offset;
        }
        self.temp_offset
    }

    fn free_temp(&mut self) {
        self.temp_offset -= 8;
    }

    fn alloc_temp_block(&mut self, count: usize) -> i32 {
        if count == 0 {
            return 0;
        }
        let bytes = (count as i32) * 8;
        self.temp_offset += bytes;
        if self.temp_offset > self.max_temp_offset {
            self.max_temp_offset = self.temp_offset;
        }
        self.temp_offset
    }

    fn free_temp_block(&mut self, count: usize) {
        self.temp_offset -= (count as i32) * 8;
    }
}

pub struct Codegen {
    rodata: String,
    functions: Vec<String>,
    str_counter: usize,
    label_counter: usize,
    fn_counter: usize,
    str_pool: HashMap<String, String>,
}

impl Codegen {
    pub fn new() -> Self {
        Self {
            rodata: String::new(),
            functions: Vec::new(),
            str_counter: 0,
            label_counter: 0,
            fn_counter: 0,
            str_pool: HashMap::new(),
        }
    }

    fn new_label(&mut self, prefix: &str) -> String {
        let id = self.label_counter;
        self.label_counter += 1;
        format!(".L{}_{}", prefix, id)
    }

    fn new_fn_label(&mut self, prefix: &str) -> String {
        let id = self.fn_counter;
        self.fn_counter += 1;
        format!("_rb_fn_{}_{}", prefix, id)
    }

    fn intern_cstr(&mut self, s: &str) -> String {
        if let Some(lbl) = self.str_pool.get(s) {
            return lbl.clone();
        }
        let lbl = format!(".LC_str_{}", self.str_counter);
        self.str_counter += 1;
        self.str_pool.insert(s.to_string(), lbl.clone());
        let mut escaped = String::new();
        for b in s.bytes() {
            match b {
                b'\\' => escaped.push_str("\\\\"),
                b'"' => escaped.push_str("\\\""),
                b'\n' => escaped.push_str("\\n"),
                b'\r' => escaped.push_str("\\r"),
                b'\t' => escaped.push_str("\\t"),
                0x20..=0x7e => escaped.push(b as char),
                other => write!(escaped, "\\{:03o}", other).unwrap(),
            }
        }
        writeln!(self.rodata, "{}:", lbl).unwrap();
        writeln!(self.rodata, "    .string \"{}\"", escaped).unwrap();
        lbl
    }

    pub fn generate(mut self, prog: &Program) -> String {
        let main_fn_label = "_rb_main_body".to_string();
        self.compile_function(
            &main_fn_label,
            &[],
            &prog.stmts,
            None,
            None,
            false,
            vec![],
        );

        let mut out = String::new();
        writeln!(out, ".intel_syntax noprefix").unwrap();
        writeln!(out, ".section .rodata").unwrap();
        out.push_str(&self.rodata);
        writeln!(out, ".text").unwrap();
        writeln!(out, ".globl main").unwrap();
        writeln!(out, "main:").unwrap();
        writeln!(out, "    push rbp").unwrap();
        writeln!(out, "    mov rbp, rsp").unwrap();
        writeln!(out, "    call rb_init@PLT").unwrap();
        writeln!(out, "    call rb_top_self@PLT").unwrap();
        writeln!(out, "    mov rsi, rax").unwrap();
        writeln!(out, "    xor rdi, rdi").unwrap();
        writeln!(out, "    xor rdx, rdx").unwrap();
        writeln!(out, "    xor rcx, rcx").unwrap();
        writeln!(out, "    mov r8, 4").unwrap(); // QNIL
        writeln!(out, "    call {}", main_fn_label).unwrap();
        writeln!(out, "    call rb_shutdown@PLT").unwrap();
        writeln!(out, "    xor eax, eax").unwrap();
        writeln!(out, "    pop rbp").unwrap();
        writeln!(out, "    ret").unwrap();
        writeln!(out).unwrap();

        for f in &self.functions {
            out.push_str(f);
            out.push('\n');
        }
        writeln!(out, ".section .note.GNU-stack,\"\",@progbits").unwrap();
        out
    }

    fn collect_vars_in_stmts(
        stmts: &[Expr],
        local_defs: &mut HashSet<String>,
        nested_uses: &mut HashSet<String>,
        in_nested: bool,
    ) {
        for s in stmts {
            Self::collect_vars_in_expr(s, local_defs, nested_uses, in_nested);
        }
    }

    fn collect_vars_in_expr(
        expr: &Expr,
        local_defs: &mut HashSet<String>,
        nested_uses: &mut HashSet<String>,
        in_nested: bool,
    ) {
        match expr {
            Expr::LocalVar(name) => {
                if in_nested {
                    nested_uses.insert(name.clone());
                } else {
                    local_defs.insert(name.clone());
                }
            }
            Expr::Assign { target, value, .. } => {
                if let AssignTarget::Local(name) = target {
                    if in_nested {
                        nested_uses.insert(name.clone());
                    } else {
                        local_defs.insert(name.clone());
                    }
                }
                Self::collect_vars_in_expr(value, local_defs, nested_uses, in_nested);
            }
            Expr::MultiAssign { targets, values } => {
                for t in targets {
                    if let AssignTarget::Local(name) = t {
                        if in_nested {
                            nested_uses.insert(name.clone());
                        } else {
                            local_defs.insert(name.clone());
                        }
                    }
                }
                Self::collect_vars_in_stmts(values, local_defs, nested_uses, in_nested);
            }
            Expr::StringLit(parts) => {
                for p in parts {
                    if let StringPart::Interp(e) = p {
                        Self::collect_vars_in_expr(e, local_defs, nested_uses, in_nested);
                    }
                }
            }
            Expr::Array(items) => Self::collect_vars_in_stmts(items, local_defs, nested_uses, in_nested),
            Expr::Hash(pairs) => {
                for (k, v) in pairs {
                    Self::collect_vars_in_expr(k, local_defs, nested_uses, in_nested);
                    Self::collect_vars_in_expr(v, local_defs, nested_uses, in_nested);
                }
            }
            Expr::Range { start, end, .. } => {
                Self::collect_vars_in_expr(start, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_expr(end, local_defs, nested_uses, in_nested);
            }
            Expr::ConstPath(e, _) | Expr::UnaryOp { expr: e, .. } => {
                Self::collect_vars_in_expr(e, local_defs, nested_uses, in_nested);
            }
            Expr::BinOp { left, right, .. } => {
                Self::collect_vars_in_expr(left, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_expr(right, local_defs, nested_uses, in_nested);
            }
            Expr::If { cond, then_body, elsifs, else_body } => {
                Self::collect_vars_in_expr(cond, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_stmts(then_body, local_defs, nested_uses, in_nested);
                for (c, b) in elsifs {
                    Self::collect_vars_in_expr(c, local_defs, nested_uses, in_nested);
                    Self::collect_vars_in_stmts(b, local_defs, nested_uses, in_nested);
                }
                if let Some(eb) = else_body {
                    Self::collect_vars_in_stmts(eb, local_defs, nested_uses, in_nested);
                }
            }
            Expr::While { cond, body, .. } => {
                Self::collect_vars_in_expr(cond, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_stmts(body, local_defs, nested_uses, in_nested);
            }
            Expr::For { var, iter, body } => {
                if in_nested {
                    nested_uses.insert(var.clone());
                } else {
                    local_defs.insert(var.clone());
                }
                Self::collect_vars_in_expr(iter, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_stmts(body, local_defs, nested_uses, true);
            }
            Expr::Case { expr, whens, else_body } => {
                if let Some(e) = expr {
                    Self::collect_vars_in_expr(e, local_defs, nested_uses, in_nested);
                }
                for (pats, body) in whens {
                    Self::collect_vars_in_stmts(pats, local_defs, nested_uses, in_nested);
                    Self::collect_vars_in_stmts(body, local_defs, nested_uses, in_nested);
                }
                if let Some(eb) = else_body {
                    Self::collect_vars_in_stmts(eb, local_defs, nested_uses, in_nested);
                }
            }
            Expr::Break(Some(e)) | Expr::Next(Some(e)) | Expr::Return(Some(e)) => {
                Self::collect_vars_in_expr(e, local_defs, nested_uses, in_nested);
            }
            Expr::Yield(args) => Self::collect_vars_in_stmts(args, local_defs, nested_uses, in_nested),
            Expr::Super(args) => {
                for a in args {
                    Self::collect_vars_in_expr(&a.expr, local_defs, nested_uses, in_nested);
                }
            }
            Expr::Call { receiver, args, block, .. } => {
                if let Some(r) = receiver {
                    Self::collect_vars_in_expr(r, local_defs, nested_uses, in_nested);
                }
                for a in args {
                    Self::collect_vars_in_expr(&a.expr, local_defs, nested_uses, in_nested);
                }
                if let Some(b) = block {
                    Self::collect_vars_in_stmts(&b.body, local_defs, nested_uses, true);
                }
            }
            Expr::Index { receiver, args } => {
                Self::collect_vars_in_expr(receiver, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_stmts(args, local_defs, nested_uses, in_nested);
            }
            Expr::IndexAssign { receiver, args, value, .. } => {
                Self::collect_vars_in_expr(receiver, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_stmts(args, local_defs, nested_uses, in_nested);
                Self::collect_vars_in_expr(value, local_defs, nested_uses, in_nested);
            }
            Expr::Lambda { body, .. } => {
                Self::collect_vars_in_stmts(body, local_defs, nested_uses, true);
            }
            Expr::Begin { body, rescues, ensure } => {
                Self::collect_vars_in_stmts(body, local_defs, nested_uses, in_nested);
                for r in rescues {
                    if let Some(v) = &r.var {
                        if in_nested {
                            nested_uses.insert(v.clone());
                        } else {
                            local_defs.insert(v.clone());
                        }
                    }
                    Self::collect_vars_in_stmts(&r.body, local_defs, nested_uses, in_nested);
                }
                if let Some(ens) = ensure {
                    Self::collect_vars_in_stmts(ens, local_defs, nested_uses, in_nested);
                }
            }
            _ => {}
        }
    }

    fn compile_function(
        &mut self,
        label: &str,
        params: &[Param],
        body: &[Expr],
        defining_class_slot_val: Option<String>,
        current_method_name: Option<String>,
        is_block: bool,
        outer_scopes: Vec<ScopeInfo>,
    ) {
        let mut local_defs = HashSet::new();
        for p in params {
            local_defs.insert(p.name.clone());
        }
        let mut nested_uses = HashSet::new();
        Self::collect_vars_in_stmts(body, &mut local_defs, &mut nested_uses, false);

        // Captured vars are those defined/used in this scope AND referenced in a nested block/lambda
        let mut captured_names: Vec<String> = local_defs
            .intersection(&nested_uses)
            .cloned()
            .collect();
        captured_names.sort();

        // Also check outer scopes: only Env-captured variables are visible in inner scopes
        let mut outer_var_names = HashSet::new();
        for sc in &outer_scopes {
            for (k, loc) in &sc.locals {
                if matches!(loc, VarLoc::Env(_, _)) {
                    outer_var_names.insert(k.clone());
                }
            }
        }
        captured_names.retain(|n| !outer_var_names.contains(n));

        let epilogue_label = format!(".Lepilogue_{}", label);

        let mut st = FnState {
            next_local_offset: 0,
            temp_offset: 0,
            max_temp_offset: 0,
            epilogue_label: epilogue_label.clone(),
            loop_stack: Vec::new(),
            scopes: Vec::new(),
            self_slot: 0,
            argc_slot: 0,
            argv_slot: 0,
            block_slot: 0,
            defining_class_slot: None,
            current_method_name,
            is_block,
        };

        let env_arg_slot = st.alloc_stack(8);
        st.self_slot = st.alloc_stack(8);
        st.argc_slot = st.alloc_stack(8);
        st.argv_slot = st.alloc_stack(8);
        st.block_slot = st.alloc_stack(8);
        if defining_class_slot_val.is_some() {
            st.defining_class_slot = Some(st.alloc_stack(8));
        }

        // Shift outer scopes' Env depths by +1 if this scope allocates an env, or keep them if not
        let has_env = !captured_names.is_empty();
        let mut shifted_outer = Vec::new();
        for sc in outer_scopes {
            let mut new_locals = HashMap::new();
            for (k, loc) in sc.locals {
                if let VarLoc::Env(depth, idx) = loc {
                    new_locals.insert(k, VarLoc::Env(depth + if has_env { 1 } else { 0 }, idx));
                }
            }
            shifted_outer.push(ScopeInfo {
                locals: new_locals,
                captured_names: sc.captured_names,
                has_env: sc.has_env,
                parent_env_slot: None,
            });
        }

        let mut cur_locals = HashMap::new();
        let parent_env_slot = if has_env {
            let s = st.alloc_stack(8);
            for (i, name) in captured_names.iter().enumerate() {
                // Slot 0 is parent_env, captured vars start at 1
                cur_locals.insert(name.clone(), VarLoc::Env(0, i + 1));
            }
            Some(s)
        } else {
            None
        };

        let mut all_local_names: Vec<String> = local_defs.into_iter().collect();
        all_local_names.sort();
        for name in all_local_names {
            if !outer_var_names.contains(&name) && !cur_locals.contains_key(&name) {
                let off = st.alloc_stack(8);
                cur_locals.insert(name, VarLoc::Stack(off));
            }
        }

        st.scopes = shifted_outer;
        st.scopes.push(ScopeInfo {
            locals: cur_locals,
            captured_names: captured_names.clone(),
            has_env,
            parent_env_slot,
        });

        let mut prologue = String::new();
        writeln!(prologue, "{}:", label).unwrap();
        writeln!(prologue, "    push rbp").unwrap();
        writeln!(prologue, "    mov rbp, rsp").unwrap();

        let mut asm = String::new();
        writeln!(asm, "    mov [rbp - {}], rdi", env_arg_slot).unwrap();
        writeln!(asm, "    mov [rbp - {}], rsi", st.self_slot).unwrap();
        writeln!(asm, "    mov [rbp - {}], rdx", st.argc_slot).unwrap();
        writeln!(asm, "    mov [rbp - {}], rcx", st.argv_slot).unwrap();
        writeln!(asm, "    mov [rbp - {}], r8", st.block_slot).unwrap();

        if let Some(ref cls_sym) = defining_class_slot_val {
            let dslot = st.defining_class_slot.unwrap();
            writeln!(asm, "    mov rax, [rip + {}]", cls_sym).unwrap();
            writeln!(asm, "    mov [rbp - {}], rax", dslot).unwrap();
        }

        if has_env {
            let pslot = parent_env_slot.unwrap();
            writeln!(asm, "    mov rdi, {}", captured_names.len() + 1).unwrap();
            writeln!(asm, "    call rb_env_alloc@PLT").unwrap();
            writeln!(asm, "    mov [rbp - {}], rax", pslot).unwrap();
            writeln!(asm, "    mov rdi, rax").unwrap();
            writeln!(asm, "    xor rsi, rsi").unwrap();
            writeln!(asm, "    mov rdx, [rbp - {}]", env_arg_slot).unwrap();
            writeln!(asm, "    call rb_env_set@PLT").unwrap();
        }

        // Initialize all stack locals to QNIL (4)
        let cur_scope = st.scopes.last().unwrap().locals.clone();
        for (_, loc) in &cur_scope {
            if let VarLoc::Stack(off) = loc {
                writeln!(asm, "    mov qword ptr [rbp - {}], 4", off).unwrap();
            }
        }

        // Bind parameters
        self.emit_bind_params(&mut asm, &mut st, params, env_arg_slot);

        // Compile body
        if body.is_empty() {
            writeln!(asm, "    mov rax, 4").unwrap();
        } else {
            for expr in body {
                self.emit_expr(&mut asm, &mut st, expr, env_arg_slot);
            }
        }

        writeln!(asm, "{}:", epilogue_label).unwrap();
        writeln!(asm, "    leave").unwrap();
        writeln!(asm, "    ret").unwrap();
        let aligned_frame = ((st.max_temp_offset + 15) / 16) * 16;
        if aligned_frame > 0 {
            writeln!(prologue, "    sub rsp, {}", aligned_frame).unwrap();
        }
        prologue.push_str(&asm);

        self.functions.push(prologue);
    }

    fn emit_bind_params(&mut self, asm: &mut String, st: &mut FnState, params: &[Param], env_arg_slot: i32) {
        let mut idx = 0usize;
        for p in params {
            if p.is_block {
                writeln!(asm, "    mov rax, [rbp - {}]", st.block_slot).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
            } else if p.is_splat {
                // Collect remaining args from idx..argc into an Array
                let lbl_ok = self.new_label("splat_ok");
                let lbl_done = self.new_label("splat_done");
                writeln!(asm, "    mov rax, [rbp - {}]", st.argc_slot).unwrap();
                writeln!(asm, "    sub rax, {}", idx).unwrap();
                writeln!(asm, "    cmp rax, 0").unwrap();
                writeln!(asm, "    jg {}", lbl_ok).unwrap();
                writeln!(asm, "    xor rdi, rdi").unwrap();
                writeln!(asm, "    xor rsi, rsi").unwrap();
                writeln!(asm, "    call rb_ary_new@PLT").unwrap();
                writeln!(asm, "    jmp {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_ok).unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    mov rsi, [rbp - {}]", st.argv_slot).unwrap();
                writeln!(asm, "    add rsi, {}", idx * 8).unwrap();
                writeln!(asm, "    call rb_ary_new@PLT").unwrap();
                writeln!(asm, "{}:", lbl_done).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
            } else if let Some(ref def_expr) = p.default {
                let lbl_def = self.new_label("param_def");
                let lbl_done = self.new_label("param_done");
                writeln!(asm, "    mov rax, [rbp - {}]", st.argc_slot).unwrap();
                writeln!(asm, "    cmp rax, {}", idx).unwrap();
                writeln!(asm, "    jle {}", lbl_def).unwrap();
                writeln!(asm, "    mov rcx, [rbp - {}]", st.argv_slot).unwrap();
                writeln!(asm, "    mov rax, [rcx + {}]", idx * 8).unwrap();
                writeln!(asm, "    jmp {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_def).unwrap();
                self.emit_expr(asm, st, def_expr, env_arg_slot);
                writeln!(asm, "{}:", lbl_done).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
                idx += 1;
            } else {
                let lbl_nil = self.new_label("param_nil");
                let lbl_done = self.new_label("param_done");
                writeln!(asm, "    mov rax, [rbp - {}]", st.argc_slot).unwrap();
                writeln!(asm, "    cmp rax, {}", idx).unwrap();
                writeln!(asm, "    jle {}", lbl_nil).unwrap();
                writeln!(asm, "    mov rcx, [rbp - {}]", st.argv_slot).unwrap();
                writeln!(asm, "    mov rax, [rcx + {}]", idx * 8).unwrap();
                writeln!(asm, "    jmp {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_nil).unwrap();
                writeln!(asm, "    mov rax, 4").unwrap(); // QNIL
                writeln!(asm, "{}:", lbl_done).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
                idx += 1;
            }
        }
    }

    fn lookup_var(&self, st: &FnState, name: &str) -> Option<VarLoc> {
        for sc in st.scopes.iter().rev() {
            if let Some(loc) = sc.locals.get(name) {
                return Some(loc.clone());
            }
        }
        None
    }

    fn emit_load_env_ptr(asm: &mut String, st: &FnState, depth: usize, env_arg_slot: i32) {
        let cur_has_env = st.scopes.last().map(|s| s.has_env).unwrap_or(false);
        if cur_has_env && depth == 0 {
            let pslot = st.scopes.last().unwrap().parent_env_slot.unwrap();
            writeln!(asm, "    mov rax, [rbp - {}]", pslot).unwrap();
        } else {
            let mut hops = depth;
            if cur_has_env {
                hops -= 1;
            }
            writeln!(asm, "    mov rax, [rbp - {}]", env_arg_slot).unwrap();
            for _ in 0..hops {
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    xor rsi, rsi").unwrap();
                writeln!(asm, "    call rb_env_get@PLT").unwrap();
            }
        }
    }

    fn emit_load_local(&mut self, asm: &mut String, st: &FnState, name: &str, env_arg_slot: i32) -> bool {
        match self.lookup_var(st, name) {
            Some(VarLoc::Stack(off)) => {
                writeln!(asm, "    mov rax, [rbp - {}]", off).unwrap();
                true
            }
            Some(VarLoc::Env(depth, idx)) => {
                Self::emit_load_env_ptr(asm, st, depth, env_arg_slot);
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    mov rsi, {}", idx).unwrap();
                writeln!(asm, "    call rb_env_get@PLT").unwrap();
                true
            }
            None => false,
        }
    }

    fn emit_store_local(&mut self, asm: &mut String, st: &mut FnState, name: &str, env_arg_slot: i32) {
        if let Some(loc) = self.lookup_var(st, name) {
            match loc {
                VarLoc::Stack(off) => {
                    writeln!(asm, "    mov [rbp - {}], rax", off).unwrap();
                }
                VarLoc::Env(depth, idx) => {
                    let t = st.alloc_temp();
                    writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                    Self::emit_load_env_ptr(asm, st, depth, env_arg_slot);
                    writeln!(asm, "    mov rdi, rax").unwrap();
                    writeln!(asm, "    mov rsi, {}", idx).unwrap();
                    writeln!(asm, "    mov rdx, [rbp - {}]", t).unwrap();
                    writeln!(asm, "    call rb_env_set@PLT").unwrap();
                    writeln!(asm, "    mov rax, [rbp - {}]", t).unwrap();
                    st.free_temp();
                }
            }
        }
    }

    fn emit_expr(&mut self, asm: &mut String, st: &mut FnState, expr: &Expr, env_arg_slot: i32) {
        match expr {
            Expr::Int(n) => {
                let tagged = ((*n as i64) << 1) | 1;
                writeln!(asm, "    mov rax, {}", tagged).unwrap();
            }
            Expr::Float(f) => {
                let bits = f.to_bits();
                writeln!(asm, "    mov rax, {}", bits).unwrap();
                writeln!(asm, "    movq xmm0, rax").unwrap();
                writeln!(asm, "    call rb_float_new@PLT").unwrap();
            }
            Expr::Bool(true) => writeln!(asm, "    mov rax, 2").unwrap(),
            Expr::Bool(false) => writeln!(asm, "    xor eax, eax").unwrap(),
            Expr::Nil => writeln!(asm, "    mov rax, 4").unwrap(),
            Expr::SelfExpr => writeln!(asm, "    mov rax, [rbp - {}]", st.self_slot).unwrap(),

            Expr::Symbol(s) => {
                let lbl = self.intern_cstr(s);
                writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_sym_intern@PLT").unwrap();
            }

            Expr::StringLit(parts) => {
                if parts.is_empty() {
                    let lbl = self.intern_cstr("");
                    writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                    writeln!(asm, "    xor rsi, rsi").unwrap();
                    writeln!(asm, "    call rb_str_new@PLT").unwrap();
                } else if parts.len() == 1 {
                    match &parts[0] {
                        StringPart::Literal(s) => {
                            let lbl = self.intern_cstr(s);
                            writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                            writeln!(asm, "    mov rsi, {}", s.len()).unwrap();
                            writeln!(asm, "    call rb_str_new@PLT").unwrap();
                        }
                        StringPart::Interp(e) => {
                            self.emit_expr(asm, st, e, env_arg_slot);
                            writeln!(asm, "    mov rdi, rax").unwrap();
                            writeln!(asm, "    call rb_to_s@PLT").unwrap();
                        }
                    }
                } else {
                    let n = parts.len();
                    let base = st.alloc_temp_block(n);
                    for (i, p) in parts.iter().enumerate() {
                        match p {
                            StringPart::Literal(s) => {
                                let lbl = self.intern_cstr(s);
                                writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                                writeln!(asm, "    mov rsi, {}", s.len()).unwrap();
                                writeln!(asm, "    call rb_str_new@PLT").unwrap();
                            }
                            StringPart::Interp(e) => {
                                self.emit_expr(asm, st, e, env_arg_slot);
                            }
                        }
                        writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                    }
                    writeln!(asm, "    mov rdi, {}", n).unwrap();
                    writeln!(asm, "    lea rsi, [rbp - {}]", base).unwrap();
                    writeln!(asm, "    call rb_str_interp@PLT").unwrap();
                    st.free_temp_block(n);
                }
            }

            Expr::Array(items) => {
                let n = items.len();
                if n == 0 {
                    writeln!(asm, "    xor rdi, rdi").unwrap();
                    writeln!(asm, "    xor rsi, rsi").unwrap();
                    writeln!(asm, "    call rb_ary_new@PLT").unwrap();
                } else {
                    let base = st.alloc_temp_block(n);
                    for (i, item) in items.iter().enumerate() {
                        self.emit_expr(asm, st, item, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                    }
                    writeln!(asm, "    mov rdi, {}", n).unwrap();
                    writeln!(asm, "    lea rsi, [rbp - {}]", base).unwrap();
                    writeln!(asm, "    call rb_ary_new@PLT").unwrap();
                    st.free_temp_block(n);
                }
            }

            Expr::Hash(pairs) => {
                let n = pairs.len();
                if n == 0 {
                    writeln!(asm, "    xor rdi, rdi").unwrap();
                    writeln!(asm, "    xor rsi, rsi").unwrap();
                    writeln!(asm, "    call rb_hash_new@PLT").unwrap();
                } else {
                    let base = st.alloc_temp_block(n * 2);
                    for (i, (k, v)) in pairs.iter().enumerate() {
                        self.emit_expr(asm, st, k, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32 * 2) * 8).unwrap();
                        self.emit_expr(asm, st, v, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32 * 2 + 1) * 8).unwrap();
                    }
                    writeln!(asm, "    mov rdi, {}", n).unwrap();
                    writeln!(asm, "    lea rsi, [rbp - {}]", base).unwrap();
                    writeln!(asm, "    call rb_hash_new@PLT").unwrap();
                    st.free_temp_block(n * 2);
                }
            }

            Expr::Range { start, end, exclusive } => {
                let t = st.alloc_temp();
                self.emit_expr(asm, st, start, env_arg_slot);
                writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                self.emit_expr(asm, st, end, env_arg_slot);
                writeln!(asm, "    mov rsi, rax").unwrap();
                writeln!(asm, "    mov rdi, [rbp - {}]", t).unwrap();
                writeln!(asm, "    mov rdx, {}", if *exclusive { 1 } else { 0 }).unwrap();
                writeln!(asm, "    call rb_range_new@PLT").unwrap();
                st.free_temp();
            }

            Expr::LocalVar(name) => {
                if !self.emit_load_local(asm, st, name, env_arg_slot) {
                    // Treat unknown bare identifier as zero-arg method call on self
                    let lbl = self.intern_cstr(name);
                    writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                    writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                    writeln!(asm, "    xor rdx, rdx").unwrap();
                    writeln!(asm, "    xor rcx, rcx").unwrap();
                    writeln!(asm, "    mov r8, 4").unwrap();
                    writeln!(asm, "    call rb_send@PLT").unwrap();
                }
            }

            Expr::Ivar(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_ivar_get@PLT").unwrap();
            }

            Expr::Cvar(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_cvar_get@PLT").unwrap();
            }

            Expr::Gvar(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_gvar_get@PLT").unwrap();
            }

            Expr::Const(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_const_get@PLT").unwrap();
            }

            Expr::ConstPath(parent, name) => {
                self.emit_expr(asm, st, parent, env_arg_slot);
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_const_get_path@PLT").unwrap();
            }

            Expr::Assign { target, op, value } => {
                if let Some(binop) = op {
                    if matches!(binop, BinOp::And | BinOp::Or) {
                        // Short-circuit &&= / ||=
                        self.emit_load_target(asm, st, target, env_arg_slot);
                        let lbl_skip = self.new_label("short_assign");
                        let t = st.alloc_temp();
                        writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                        writeln!(asm, "    mov rdi, rax").unwrap();
                        writeln!(asm, "    call rb_test@PLT").unwrap();
                        writeln!(asm, "    test eax, eax").unwrap();
                        if matches!(binop, BinOp::Or) {
                            writeln!(asm, "    jnz {}", lbl_skip).unwrap();
                        } else {
                            writeln!(asm, "    jz {}", lbl_skip).unwrap();
                        }
                        self.emit_expr(asm, st, value, env_arg_slot);
                        self.emit_store_target(asm, st, target, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                        writeln!(asm, "{}:", lbl_skip).unwrap();
                        writeln!(asm, "    mov rax, [rbp - {}]", t).unwrap();
                        st.free_temp();
                        return;
                    }
                    let t = st.alloc_temp();
                    self.emit_load_target(asm, st, target, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                    self.emit_expr(asm, st, value, env_arg_slot);
                    self.emit_binop_call(asm, binop, t);
                    st.free_temp();
                } else {
                    self.emit_expr(asm, st, value, env_arg_slot);
                }
                self.emit_store_target(asm, st, target, env_arg_slot);
            }

            Expr::MultiAssign { targets, values } => {
                if values.len() == 1 {
                    self.emit_expr(asm, st, &values[0], env_arg_slot);
                    let t = st.alloc_temp();
                    writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                    for (i, tgt) in targets.iter().enumerate() {
                        writeln!(asm, "    mov rdi, [rbp - {}]", t).unwrap();
                        writeln!(asm, "    mov rsi, {}", i).unwrap();
                        writeln!(asm, "    call rb_ary_get@PLT").unwrap();
                        self.emit_store_target(asm, st, tgt, env_arg_slot);
                    }
                    writeln!(asm, "    mov rax, [rbp - {}]", t).unwrap();
                    st.free_temp();
                } else {
                    let n = values.len();
                    let base = st.alloc_temp_block(n);
                    for (i, v) in values.iter().enumerate() {
                        self.emit_expr(asm, st, v, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                    }
                    for (i, tgt) in targets.iter().enumerate() {
                        if i < n {
                            writeln!(asm, "    mov rax, [rbp - {}]", base - (i as i32) * 8).unwrap();
                        } else {
                            writeln!(asm, "    mov rax, 4").unwrap();
                        }
                        self.emit_store_target(asm, st, tgt, env_arg_slot);
                    }
                    st.free_temp_block(n);
                }
            }

            Expr::BinOp { op, left, right } => {
                match op {
                    BinOp::And => {
                        let lbl_end = self.new_label("and_end");
                        self.emit_expr(asm, st, left, env_arg_slot);
                        let t = st.alloc_temp();
                        writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                        writeln!(asm, "    mov rdi, rax").unwrap();
                        writeln!(asm, "    call rb_test@PLT").unwrap();
                        writeln!(asm, "    test eax, eax").unwrap();
                        writeln!(asm, "    mov rax, [rbp - {}]", t).unwrap();
                        writeln!(asm, "    jz {}", lbl_end).unwrap();
                        self.emit_expr(asm, st, right, env_arg_slot);
                        writeln!(asm, "{}:", lbl_end).unwrap();
                        st.free_temp();
                    }
                    BinOp::Or => {
                        let lbl_end = self.new_label("or_end");
                        self.emit_expr(asm, st, left, env_arg_slot);
                        let t = st.alloc_temp();
                        writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                        writeln!(asm, "    mov rdi, rax").unwrap();
                        writeln!(asm, "    call rb_test@PLT").unwrap();
                        writeln!(asm, "    test eax, eax").unwrap();
                        writeln!(asm, "    mov rax, [rbp - {}]", t).unwrap();
                        writeln!(asm, "    jnz {}", lbl_end).unwrap();
                        self.emit_expr(asm, st, right, env_arg_slot);
                        writeln!(asm, "{}:", lbl_end).unwrap();
                        st.free_temp();
                    }
                    _ => {
                        let t = st.alloc_temp();
                        self.emit_expr(asm, st, left, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                        self.emit_expr(asm, st, right, env_arg_slot);
                        self.emit_binop_call(asm, op, t);
                        st.free_temp();
                    }
                }
            }

            Expr::UnaryOp { op, expr } => {
                self.emit_expr(asm, st, expr, env_arg_slot);
                writeln!(asm, "    mov rdi, rax").unwrap();
                match op {
                    UnaryOp::Neg => writeln!(asm, "    call rb_neg@PLT").unwrap(),
                    UnaryOp::Pos => writeln!(asm, "    call rb_pos@PLT").unwrap(),
                    UnaryOp::Not => writeln!(asm, "    call rb_not@PLT").unwrap(),
                    UnaryOp::BitNot => writeln!(asm, "    call rb_bitnot@PLT").unwrap(),
                }
            }

            Expr::If { cond, then_body, elsifs, else_body } => {
                let lbl_end = self.new_label("if_end");
                let mut lbl_next = self.new_label("if_next");

                self.emit_expr(asm, st, cond, env_arg_slot);
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    call rb_test@PLT").unwrap();
                writeln!(asm, "    test eax, eax").unwrap();
                writeln!(asm, "    jz {}", lbl_next).unwrap();

                self.emit_body(asm, st, then_body, env_arg_slot);
                writeln!(asm, "    jmp {}", lbl_end).unwrap();

                for (e_cond, e_body) in elsifs {
                    writeln!(asm, "{}:", lbl_next).unwrap();
                    lbl_next = self.new_label("elsif_next");
                    self.emit_expr(asm, st, e_cond, env_arg_slot);
                    writeln!(asm, "    mov rdi, rax").unwrap();
                    writeln!(asm, "    call rb_test@PLT").unwrap();
                    writeln!(asm, "    test eax, eax").unwrap();
                    writeln!(asm, "    jz {}", lbl_next).unwrap();
                    self.emit_body(asm, st, e_body, env_arg_slot);
                    writeln!(asm, "    jmp {}", lbl_end).unwrap();
                }

                writeln!(asm, "{}:", lbl_next).unwrap();
                if let Some(eb) = else_body {
                    self.emit_body(asm, st, eb, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
                writeln!(asm, "{}:", lbl_end).unwrap();
            }

            Expr::While { cond, body, is_until } => {
                let lbl_cond = self.new_label("while_cond");
                let lbl_end = self.new_label("while_end");
                st.loop_stack.push((lbl_cond.clone(), lbl_end.clone()));

                writeln!(asm, "{}:", lbl_cond).unwrap();
                self.emit_expr(asm, st, cond, env_arg_slot);
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    call rb_test@PLT").unwrap();
                writeln!(asm, "    test eax, eax").unwrap();
                if *is_until {
                    writeln!(asm, "    jnz {}", lbl_end).unwrap();
                } else {
                    writeln!(asm, "    jz {}", lbl_end).unwrap();
                }
                for s in body {
                    self.emit_expr(asm, st, s, env_arg_slot);
                }
                writeln!(asm, "    jmp {}", lbl_cond).unwrap();
                writeln!(asm, "{}:", lbl_end).unwrap();
                writeln!(asm, "    mov rax, 4").unwrap();
                st.loop_stack.pop();
            }

            Expr::For { var, iter, body } => {
                // Desugar `for var in iter` into `iter.each { |var| body }`
                let block = Block {
                    params: vec![Param { name: var.clone(), default: None, is_splat: false, is_block: false }],
                    body: body.clone(),
                };
                let call = Expr::Call {
                    receiver: Some(iter.clone()),
                    name: "each".to_string(),
                    args: vec![],
                    block: Some(block),
                    safe_nav: false,
                };
                self.emit_expr(asm, st, &call, env_arg_slot);
            }

            Expr::Case { expr, whens, else_body } => {
                let lbl_end = self.new_label("case_end");
                let target_slot = if let Some(e) = expr {
                    let t = st.alloc_temp();
                    self.emit_expr(asm, st, e, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                    Some(t)
                } else {
                    None
                };

                for (pats, body) in whens {
                    let lbl_body = self.new_label("when_body");
                    let lbl_next = self.new_label("when_next");
                    for p in pats {
                        self.emit_expr(asm, st, p, env_arg_slot);
                        if let Some(ts) = target_slot {
                            writeln!(asm, "    mov rdi, rax").unwrap();
                            writeln!(asm, "    mov rsi, [rbp - {}]", ts).unwrap();
                            writeln!(asm, "    call rb_case_eq@PLT").unwrap();
                        }
                        writeln!(asm, "    mov rdi, rax").unwrap();
                        writeln!(asm, "    call rb_test@PLT").unwrap();
                        writeln!(asm, "    test eax, eax").unwrap();
                        writeln!(asm, "    jnz {}", lbl_body).unwrap();
                    }
                    writeln!(asm, "    jmp {}", lbl_next).unwrap();
                    writeln!(asm, "{}:", lbl_body).unwrap();
                    self.emit_body(asm, st, body, env_arg_slot);
                    writeln!(asm, "    jmp {}", lbl_end).unwrap();
                    writeln!(asm, "{}:", lbl_next).unwrap();
                }

                if let Some(eb) = else_body {
                    self.emit_body(asm, st, eb, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
                writeln!(asm, "{}:", lbl_end).unwrap();
                if target_slot.is_some() {
                    st.free_temp();
                }
            }

            Expr::Break(val) => {
                if let Some(v) = val {
                    self.emit_expr(asm, st, v, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
                if let Some((_, brk)) = st.loop_stack.last() {
                    writeln!(asm, "    jmp {}", brk).unwrap();
                } else {
                    writeln!(asm, "    jmp {}", st.epilogue_label).unwrap();
                }
            }

            Expr::Next(val) => {
                if let Some(v) = val {
                    self.emit_expr(asm, st, v, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
                if let Some((cont, _)) = st.loop_stack.last() {
                    writeln!(asm, "    jmp {}", cont).unwrap();
                } else if st.is_block {
                    writeln!(asm, "    jmp {}", st.epilogue_label).unwrap();
                }
            }

            Expr::Return(val) => {
                if let Some(v) = val {
                    self.emit_expr(asm, st, v, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
                writeln!(asm, "    jmp {}", st.epilogue_label).unwrap();
            }

            Expr::Yield(args) => {
                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, a, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                }
                writeln!(asm, "    mov rdi, [rbp - {}]", st.block_slot).unwrap();
                writeln!(asm, "    mov rsi, {}", n).unwrap();
                if n > 0 {
                    writeln!(asm, "    lea rdx, [rbp - {}]", base).unwrap();
                } else {
                    writeln!(asm, "    xor rdx, rdx").unwrap();
                }
                writeln!(asm, "    mov rcx, 4").unwrap();
                writeln!(asm, "    call rb_proc_call@PLT").unwrap();
                if n > 0 {
                    st.free_temp_block(n);
                }
            }

            Expr::Super(args) => {
                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, &a.expr, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                }
                let mname = st.current_method_name.clone().unwrap_or_else(|| "initialize".to_string());
                let mlbl = self.intern_cstr(&mname);
                if let Some(dslot) = st.defining_class_slot {
                    writeln!(asm, "    mov rdi, [rbp - {}]", dslot).unwrap();
                } else {
                    writeln!(asm, "    xor rdi, rdi").unwrap();
                }
                writeln!(asm, "    mov rsi, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    lea rdx, [rip + {}]", mlbl).unwrap();
                writeln!(asm, "    mov rcx, {}", n).unwrap();
                if n > 0 {
                    writeln!(asm, "    lea r8, [rbp - {}]", base).unwrap();
                } else {
                    writeln!(asm, "    xor r8, r8").unwrap();
                }
                writeln!(asm, "    mov r9, [rbp - {}]", st.block_slot).unwrap();
                writeln!(asm, "    call rb_send_super@PLT").unwrap();
                if n > 0 {
                    st.free_temp_block(n);
                }
            }

            Expr::Call { receiver, name, args, block, safe_nav } => {
                // Handle special class-level DSL calls: attr_reader, attr_writer, attr_accessor, include
                if receiver.is_none() && (name == "attr_reader" || name == "attr_writer" || name == "attr_accessor") {
                    for a in args {
                        if let Expr::Symbol(sym) = &a.expr {
                            if name == "attr_reader" || name == "attr_accessor" {
                                let getter = Expr::Def {
                                    singleton: None,
                                    name: sym.clone(),
                                    params: vec![],
                                    body: vec![Expr::Ivar(format!("@{}", sym))],
                                };
                                self.emit_expr(asm, st, &getter, env_arg_slot);
                            }
                            if name == "attr_writer" || name == "attr_accessor" {
                                let setter = Expr::Def {
                                    singleton: None,
                                    name: format!("{}=", sym),
                                    params: vec![Param { name: "val".to_string(), default: None, is_splat: false, is_block: false }],
                                    body: vec![Expr::Assign {
                                        target: AssignTarget::Ivar(format!("@{}", sym)),
                                        op: None,
                                        value: Box::new(Expr::LocalVar("val".to_string())),
                                    }],
                                };
                                self.emit_expr(asm, st, &setter, env_arg_slot);
                            }
                        }
                    }
                    writeln!(asm, "    mov rax, 4").unwrap();
                    return;
                }
                if receiver.is_none() && name == "include" && args.len() == 1 {
                    self.emit_expr(asm, st, &args[0].expr, env_arg_slot);
                    writeln!(asm, "    mov rsi, rax").unwrap();
                    writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                    writeln!(asm, "    call rb_include_module@PLT").unwrap();
                    writeln!(asm, "    mov rax, [rbp - {}]", st.self_slot).unwrap();
                    return;
                }
                if receiver.is_none() && name == "lambda" && block.is_some() {
                    let blk = block.as_ref().unwrap();
                    let lam = Expr::Lambda { params: blk.params.clone(), body: blk.body.clone() };
                    self.emit_expr(asm, st, &lam, env_arg_slot);
                    return;
                }

                let recv_slot = st.alloc_temp();
                if let Some(r) = receiver {
                    self.emit_expr(asm, st, r, env_arg_slot);
                } else {
                    writeln!(asm, "    mov rax, [rbp - {}]", st.self_slot).unwrap();
                }
                writeln!(asm, "    mov [rbp - {}], rax", recv_slot).unwrap();

                let lbl_nil_skip = if *safe_nav {
                    let l = self.new_label("safe_nav_nil");
                    writeln!(asm, "    cmp rax, 4").unwrap();
                    writeln!(asm, "    je {}", l).unwrap();
                    Some(l)
                } else {
                    None
                };

                let blk_slot = st.alloc_temp();
                if let Some(blk) = block {
                    let blk_fn = self.new_fn_label("block");
                    self.compile_function(
                        &blk_fn,
                        &blk.params,
                        &blk.body,
                        None,
                        st.current_method_name.clone(),
                        true,
                        st.scopes.clone(),
                    );
                    writeln!(asm, "    lea rdi, [rip + {}]", blk_fn).unwrap();
                    let cur_has_env = st.scopes.last().map(|s| s.has_env).unwrap_or(false);
                    if cur_has_env {
                        let pslot = st.scopes.last().unwrap().parent_env_slot.unwrap();
                        writeln!(asm, "    mov rsi, [rbp - {}]", pslot).unwrap();
                    } else {
                        writeln!(asm, "    mov rsi, [rbp - {}]", env_arg_slot).unwrap();
                    }
                    writeln!(asm, "    mov rdx, [rbp - {}]", st.self_slot).unwrap();
                    writeln!(asm, "    mov rcx, {}", blk.params.len()).unwrap();
                    writeln!(asm, "    call rb_proc_new@PLT").unwrap();
                    writeln!(asm, "    mov [rbp - {}], rax", blk_slot).unwrap();
                } else {
                    writeln!(asm, "    mov qword ptr [rbp - {}], 4", blk_slot).unwrap();
                }

                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, &a.expr, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", base - (i as i32) * 8).unwrap();
                }

                let mlbl = self.intern_cstr(name);
                writeln!(asm, "    mov rdi, [rbp - {}]", recv_slot).unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", mlbl).unwrap();
                writeln!(asm, "    mov rdx, {}", n).unwrap();
                if n > 0 {
                    writeln!(asm, "    lea rcx, [rbp - {}]", base).unwrap();
                } else {
                    writeln!(asm, "    xor rcx, rcx").unwrap();
                }
                writeln!(asm, "    mov r8, [rbp - {}]", blk_slot).unwrap();
                writeln!(asm, "    call rb_send@PLT").unwrap();

                if n > 0 {
                    st.free_temp_block(n);
                }
                st.free_temp(); // blk_slot
                if let Some(l) = lbl_nil_skip {
                    writeln!(asm, "{}:", l).unwrap();
                }
                st.free_temp(); // recv_slot
            }

            Expr::Index { receiver, args } => {
                let call = Expr::Call {
                    receiver: Some(receiver.clone()),
                    name: "[]".to_string(),
                    args: args.iter().map(|e| CallArg { expr: e.clone(), is_splat: false }).collect(),
                    block: None,
                    safe_nav: false,
                };
                self.emit_expr(asm, st, &call, env_arg_slot);
            }

            Expr::IndexAssign { receiver, args, op: _, value } => {
                let mut call_args: Vec<CallArg> = args.iter().map(|e| CallArg { expr: e.clone(), is_splat: false }).collect();
                call_args.push(CallArg { expr: (*value.clone()), is_splat: false });
                let call = Expr::Call {
                    receiver: Some(receiver.clone()),
                    name: "[]=".to_string(),
                    args: call_args,
                    block: None,
                    safe_nav: false,
                };
                self.emit_expr(asm, st, &call, env_arg_slot);
            }

            Expr::Lambda { params, body } => {
                let lam_fn = self.new_fn_label("lambda");
                self.compile_function(
                    &lam_fn,
                    params,
                    body,
                    None,
                    st.current_method_name.clone(),
                    true,
                    st.scopes.clone(),
                );
                writeln!(asm, "    lea rdi, [rip + {}]", lam_fn).unwrap();
                let cur_has_env = st.scopes.last().map(|s| s.has_env).unwrap_or(false);
                if cur_has_env {
                    let pslot = st.scopes.last().unwrap().parent_env_slot.unwrap();
                    writeln!(asm, "    mov rsi, [rbp - {}]", pslot).unwrap();
                } else {
                    writeln!(asm, "    mov rsi, [rbp - {}]", env_arg_slot).unwrap();
                }
                writeln!(asm, "    mov rdx, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    mov rcx, {}", params.len()).unwrap();
                writeln!(asm, "    call rb_proc_new@PLT").unwrap();
            }

            Expr::Def { singleton, name, params, body } => {
                let fn_lbl = self.new_fn_label(&format!("m_{}", name.replace(|c: char| !c.is_ascii_alphanumeric(), "_")));
                self.compile_function(
                    &fn_lbl,
                    params,
                    body,
                    None,
                    Some(name.clone()),
                    false,
                    vec![],
                );
                let mlbl = self.intern_cstr(name);
                if let Some(s_expr) = singleton {
                    self.emit_expr(asm, st, s_expr, env_arg_slot);
                    writeln!(asm, "    mov rdi, rax").unwrap();
                    writeln!(asm, "    lea rsi, [rip + {}]", mlbl).unwrap();
                    writeln!(asm, "    lea rdx, [rip + {}]", fn_lbl).unwrap();
                    writeln!(asm, "    mov rcx, {}", params.len()).unwrap();
                    writeln!(asm, "    call rb_def_singleton_method@PLT").unwrap();
                } else {
                    // If inside a class/module body, self IS the RClass*; at top-level, define on Object
                    let lbl_top = self.new_label("def_top");
                    let lbl_done = self.new_label("def_done");
                    writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                    writeln!(asm, "    mov rcx, [rdi]").unwrap(); // obj->base.type
                    writeln!(asm, "    cmp ecx, 6").unwrap(); // OBJ_CLASS == 6
                    writeln!(asm, "    jne {}", lbl_top).unwrap();
                    writeln!(asm, "    jmp {}", lbl_done).unwrap();
                    writeln!(asm, "{}:", lbl_top).unwrap();
                    writeln!(asm, "    call rb_object_class@PLT").unwrap();
                    writeln!(asm, "    mov rdi, rax").unwrap();
                    writeln!(asm, "{}:", lbl_done).unwrap();
                    writeln!(asm, "    lea rsi, [rip + {}]", mlbl).unwrap();
                    writeln!(asm, "    lea rdx, [rip + {}]", fn_lbl).unwrap();
                    writeln!(asm, "    mov rcx, {}", params.len()).unwrap();
                    writeln!(asm, "    call rb_def_method@PLT").unwrap();
                }
                writeln!(asm, "    lea rdi, [rip + {}]", mlbl).unwrap();
                writeln!(asm, "    call rb_sym_intern@PLT").unwrap();
            }

            Expr::Class { name, superclass, body } => {
                let super_slot = st.alloc_temp();
                if let Some(sup) = superclass {
                    self.emit_expr(asm, st, sup, env_arg_slot);
                    writeln!(asm, "    mov [rbp - {}], rax", super_slot).unwrap();
                } else {
                    writeln!(asm, "    mov qword ptr [rbp - {}], 0", super_slot).unwrap();
                }
                let clbl = self.intern_cstr(name);
                writeln!(asm, "    lea rdi, [rip + {}]", clbl).unwrap();
                writeln!(asm, "    mov rsi, [rbp - {}]", super_slot).unwrap();
                writeln!(asm, "    xor rdx, rdx").unwrap();
                writeln!(asm, "    call rb_define_class@PLT").unwrap();
                st.free_temp();

                let cls_slot = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", cls_slot).unwrap();

                let body_fn = self.new_fn_label(&format!("class_{}", name));
                self.compile_function(&body_fn, &[], body, None, None, false, vec![]);

                writeln!(asm, "    xor rdi, rdi").unwrap();
                writeln!(asm, "    mov rsi, [rbp - {}]", cls_slot).unwrap();
                writeln!(asm, "    xor rdx, rdx").unwrap();
                writeln!(asm, "    xor rcx, rcx").unwrap();
                writeln!(asm, "    mov r8, 4").unwrap();
                writeln!(asm, "    call {}", body_fn).unwrap();
                st.free_temp();
            }

            Expr::Module { name, body } => {
                let mlbl = self.intern_cstr(name);
                writeln!(asm, "    lea rdi, [rip + {}]", mlbl).unwrap();
                writeln!(asm, "    xor rsi, rsi").unwrap();
                writeln!(asm, "    call rb_define_module@PLT").unwrap();

                let mod_slot = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", mod_slot).unwrap();

                let body_fn = self.new_fn_label(&format!("mod_{}", name));
                self.compile_function(&body_fn, &[], body, None, None, false, vec![]);

                writeln!(asm, "    xor rdi, rdi").unwrap();
                writeln!(asm, "    mov rsi, [rbp - {}]", mod_slot).unwrap();
                writeln!(asm, "    xor rdx, rdx").unwrap();
                writeln!(asm, "    xor rcx, rcx").unwrap();
                writeln!(asm, "    mov r8, 4").unwrap();
                writeln!(asm, "    call {}", body_fn).unwrap();
                st.free_temp();
            }

            Expr::Begin { body, rescues, ensure } => {
                if rescues.is_empty() && ensure.is_none() {
                    self.emit_body(asm, st, body, env_arg_slot);
                    return;
                }
                // Allocate 256-byte jmp_buf on stack
                let jmp_buf_off = st.alloc_temp_block(32); // 32 * 8 = 256 bytes
                let res_slot = st.alloc_temp();
                let lbl_rescue = self.new_label("rescue");
                let lbl_ensure = self.new_label("ensure");

                writeln!(asm, "    lea rdi, [rbp - {}]", jmp_buf_off).unwrap();
                writeln!(asm, "    call rb_push_rescue@PLT").unwrap();
                writeln!(asm, "    lea rdi, [rbp - {}]", jmp_buf_off).unwrap();
                writeln!(asm, "    call _setjmp@PLT").unwrap();
                writeln!(asm, "    test eax, eax").unwrap();
                writeln!(asm, "    jnz {}", lbl_rescue).unwrap();

                self.emit_body(asm, st, body, env_arg_slot);
                writeln!(asm, "    mov [rbp - {}], rax", res_slot).unwrap();
                writeln!(asm, "    call rb_pop_rescue@PLT").unwrap();
                writeln!(asm, "    jmp {}", lbl_ensure).unwrap();

                writeln!(asm, "{}:", lbl_rescue).unwrap();
                if rescues.is_empty() {
                    // No rescue clauses, re-raise after ensure
                    if let Some(ens) = ensure {
                        self.emit_body(asm, st, ens, env_arg_slot);
                    }
                    writeln!(asm, "    call rb_current_exception@PLT").unwrap();
                    writeln!(asm, "    mov rdi, rax").unwrap();
                    writeln!(asm, "    call rb_raise@PLT").unwrap();
                } else {
                    let exc_slot = st.alloc_temp();
                    writeln!(asm, "    call rb_current_exception@PLT").unwrap();
                    writeln!(asm, "    mov [rbp - {}], rax", exc_slot).unwrap();
                    for r in rescues {
                        let lbl_match = self.new_label("rescue_match");
                        let lbl_next = self.new_label("rescue_next");
                        if r.classes.is_empty() {
                            writeln!(asm, "    jmp {}", lbl_match).unwrap();
                        } else {
                            for cls_expr in &r.classes {
                                self.emit_expr(asm, st, cls_expr, env_arg_slot);
                                writeln!(asm, "    mov rsi, rax").unwrap();
                                writeln!(asm, "    mov rdi, [rbp - {}]", exc_slot).unwrap();
                                writeln!(asm, "    call rb_exception_matches@PLT").unwrap();
                                writeln!(asm, "    test eax, eax").unwrap();
                                writeln!(asm, "    jnz {}", lbl_match).unwrap();
                            }
                            writeln!(asm, "    jmp {}", lbl_next).unwrap();
                        }
                        writeln!(asm, "{}:", lbl_match).unwrap();
                        if let Some(v) = &r.var {
                            writeln!(asm, "    mov rax, [rbp - {}]", exc_slot).unwrap();
                            self.emit_store_local(asm, st, v, env_arg_slot);
                        }
                        self.emit_body(asm, st, &r.body, env_arg_slot);
                        writeln!(asm, "    mov [rbp - {}], rax", res_slot).unwrap();
                        writeln!(asm, "    jmp {}", lbl_ensure).unwrap();
                        writeln!(asm, "{}:", lbl_next).unwrap();
                    }
                    // None matched -> re-raise
                    if let Some(ens) = ensure {
                        self.emit_body(asm, st, ens, env_arg_slot);
                    }
                    writeln!(asm, "    mov rdi, [rbp - {}]", exc_slot).unwrap();
                    writeln!(asm, "    call rb_raise@PLT").unwrap();
                    st.free_temp(); // exc_slot
                }

                writeln!(asm, "{}:", lbl_ensure).unwrap();
                if let Some(ens) = ensure {
                    self.emit_body(asm, st, ens, env_arg_slot);
                }
                writeln!(asm, "    mov rax, [rbp - {}]", res_slot).unwrap();
                st.free_temp(); // res_slot
                st.free_temp_block(32);
            }
        }
    }

    fn emit_body(&mut self, asm: &mut String, st: &mut FnState, body: &[Expr], env_arg_slot: i32) {
        if body.is_empty() {
            writeln!(asm, "    mov rax, 4").unwrap();
        } else {
            for e in body {
                self.emit_expr(asm, st, e, env_arg_slot);
            }
        }
    }

    fn emit_load_target(&mut self, asm: &mut String, st: &FnState, target: &AssignTarget, env_arg_slot: i32) {
        match target {
            AssignTarget::Local(n) => {
                if !self.emit_load_local(asm, st, n, env_arg_slot) {
                    writeln!(asm, "    mov rax, 4").unwrap();
                }
            }
            AssignTarget::Ivar(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_ivar_get@PLT").unwrap();
            }
            AssignTarget::Cvar(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_cvar_get@PLT").unwrap();
            }
            AssignTarget::Gvar(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_gvar_get@PLT").unwrap();
            }
            AssignTarget::Const(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    call rb_const_get@PLT").unwrap();
            }
        }
    }

    fn emit_store_target(&mut self, asm: &mut String, st: &mut FnState, target: &AssignTarget, env_arg_slot: i32) {
        match target {
            AssignTarget::Local(n) => self.emit_store_local(asm, st, n, env_arg_slot),
            AssignTarget::Ivar(n) => {
                let t = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    mov rdi, [rbp - {}]", st.self_slot).unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    mov rdx, [rbp - {}]", t).unwrap();
                writeln!(asm, "    call rb_ivar_set@PLT").unwrap();
                st.free_temp();
            }
            AssignTarget::Cvar(n) => {
                let t = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    mov rdx, [rbp - {}]", t).unwrap();
                writeln!(asm, "    call rb_cvar_set@PLT").unwrap();
                st.free_temp();
            }
            AssignTarget::Gvar(n) => {
                let t = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    lea rdi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    mov rsi, [rbp - {}]", t).unwrap();
                writeln!(asm, "    call rb_gvar_set@PLT").unwrap();
                st.free_temp();
            }
            AssignTarget::Const(n) => {
                let t = st.alloc_temp();
                writeln!(asm, "    mov [rbp - {}], rax", t).unwrap();
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    call rb_object_class@PLT").unwrap();
                writeln!(asm, "    mov rdi, rax").unwrap();
                writeln!(asm, "    lea rsi, [rip + {}]", lbl).unwrap();
                writeln!(asm, "    mov rdx, [rbp - {}]", t).unwrap();
                writeln!(asm, "    call rb_const_set@PLT").unwrap();
                st.free_temp();
            }
        }
    }

    fn emit_binop_call(&self, asm: &mut String, op: &BinOp, left_slot: i32) {
        writeln!(asm, "    mov rsi, rax").unwrap();
        writeln!(asm, "    mov rdi, [rbp - {}]", left_slot).unwrap();
        let func = match op {
            BinOp::Add => "rb_add",
            BinOp::Sub => "rb_sub",
            BinOp::Mul => "rb_mul",
            BinOp::Div => "rb_div",
            BinOp::Mod => "rb_mod",
            BinOp::Pow => "rb_pow",
            BinOp::Eq => "rb_eq",
            BinOp::Neq => "rb_neq",
            BinOp::Lt => "rb_lt",
            BinOp::Le => "rb_le",
            BinOp::Gt => "rb_gt",
            BinOp::Ge => "rb_ge",
            BinOp::Cmp => "rb_cmp",
            BinOp::CaseEq => "rb_case_eq",
            BinOp::BitAnd => "rb_bitand",
            BinOp::BitOr => "rb_bitor",
            BinOp::BitXor => "rb_bitxor",
            BinOp::Shl => "rb_shl",
            BinOp::Shr => "rb_shr",
            BinOp::And | BinOp::Or => unreachable!(),
        };
        writeln!(asm, "    call {}@PLT", func).unwrap();
    }
}
