use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::*;

#[derive(Clone, Debug)]
enum VarLoc {
    Stack(i32),
    Env(usize, usize),
}

#[derive(Clone)]
struct ScopeInfo {
    locals: HashMap<String, VarLoc>,
    captured_names: Vec<String>,
    has_env: bool,
    parent_env_slot: Option<i32>,
}

struct FnState {
    next_local_offset: i32,
    temp_offset: i32,
    max_temp_offset: i32,
    epilogue_label: String,
    loop_stack: Vec<(String, String)>,
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

pub struct CodegenArm64 {
    rodata: String,
    functions: Vec<String>,
    str_counter: usize,
    label_counter: usize,
    fn_counter: usize,
    str_pool: HashMap<String, String>,
}

impl CodegenArm64 {
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

    fn emit_mov_imm(asm: &mut String, reg: &str, val: u64) {
        let lo = (val & 0xFFFF) as u16;
        writeln!(asm, "    movz {}, #{}", reg, lo).unwrap();
        for shift in [16, 32, 48] {
            let chunk = ((val >> shift) & 0xFFFF) as u16;
            if chunk != 0 {
                writeln!(asm, "    movk {}, #{}, lsl #{}", reg, chunk, shift).unwrap();
            }
        }
    }

    fn emit_adr_sym(asm: &mut String, reg: &str, sym: &str) {
        writeln!(asm, "    adrp {}, {}", reg, sym).unwrap();
        writeln!(asm, "    add {}, {}, :lo12:{}", reg, reg, sym).unwrap();
    }

    fn emit_addr_fp(asm: &mut String, reg: &str, off: i32) {
        if off <= 4095 {
            writeln!(asm, "    sub {}, x29, #{}", reg, off).unwrap();
        } else {
            Self::emit_mov_imm(asm, reg, off as u64);
            writeln!(asm, "    sub {}, x29, {}", reg, reg).unwrap();
        }
    }

    fn emit_load_fp(asm: &mut String, reg: &str, off: i32) {
        if off <= 255 {
            writeln!(asm, "    ldur {}, [x29, #-{}]", reg, off).unwrap();
        } else {
            Self::emit_addr_fp(asm, "x10", off);
            writeln!(asm, "    ldr {}, [x10]", reg).unwrap();
        }
    }

    fn emit_store_fp(asm: &mut String, reg: &str, off: i32) {
        if off <= 255 {
            writeln!(asm, "    stur {}, [x29, #-{}]", reg, off).unwrap();
        } else {
            Self::emit_addr_fp(asm, "x10", off);
            writeln!(asm, "    str {}, [x10]", reg).unwrap();
        }
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
        writeln!(out, ".section .rodata").unwrap();
        out.push_str(&self.rodata);
        writeln!(out, ".text").unwrap();
        writeln!(out, ".globl main").unwrap();
        writeln!(out, "main:").unwrap();
        writeln!(out, "    stp x29, x30, [sp, #-16]!").unwrap();
        writeln!(out, "    mov x29, sp").unwrap();
        writeln!(out, "    bl rb_init").unwrap();
        writeln!(out, "    bl rb_top_self").unwrap();
        writeln!(out, "    mov x1, x0").unwrap();
        writeln!(out, "    mov x0, #0").unwrap();
        writeln!(out, "    mov x2, #0").unwrap();
        writeln!(out, "    mov x3, #0").unwrap();
        writeln!(out, "    mov x4, #4").unwrap();
        writeln!(out, "    bl {}", main_fn_label).unwrap();
        writeln!(out, "    bl rb_shutdown").unwrap();
        writeln!(out, "    mov w0, #0").unwrap();
        writeln!(out, "    ldp x29, x30, [sp], #16").unwrap();
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

        let mut captured_names: Vec<String> = local_defs
            .intersection(&nested_uses)
            .cloned()
            .collect();
        captured_names.sort();

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
        writeln!(prologue, "    stp x29, x30, [sp, #-16]!").unwrap();
        writeln!(prologue, "    mov x29, sp").unwrap();

        let mut asm = String::new();
        Self::emit_store_fp(&mut asm, "x0", env_arg_slot);
        Self::emit_store_fp(&mut asm, "x1", st.self_slot);
        Self::emit_store_fp(&mut asm, "x2", st.argc_slot);
        Self::emit_store_fp(&mut asm, "x3", st.argv_slot);
        Self::emit_store_fp(&mut asm, "x4", st.block_slot);

        if has_env {
            let pslot = parent_env_slot.unwrap();
            Self::emit_mov_imm(&mut asm, "x0", (captured_names.len() + 1) as u64);
            writeln!(asm, "    bl rb_env_alloc").unwrap();
            Self::emit_store_fp(&mut asm, "x0", pslot);
            writeln!(asm, "    mov x1, #0").unwrap();
            Self::emit_load_fp(&mut asm, "x2", env_arg_slot);
            writeln!(asm, "    bl rb_env_set").unwrap();
        }

        let cur_scope = st.scopes.last().unwrap().locals.clone();
        writeln!(asm, "    mov x9, #4").unwrap();
        for (_, loc) in &cur_scope {
            if let VarLoc::Stack(off) = loc {
                Self::emit_store_fp(&mut asm, "x9", *off);
            }
        }

        self.emit_bind_params(&mut asm, &mut st, params, env_arg_slot);

        if body.is_empty() {
            writeln!(asm, "    mov x0, #4").unwrap();
        } else {
            for expr in body {
                self.emit_expr(&mut asm, &mut st, expr, env_arg_slot);
            }
        }

        writeln!(asm, "{}:", epilogue_label).unwrap();
        writeln!(asm, "    mov sp, x29").unwrap();
        writeln!(asm, "    ldp x29, x30, [sp], #16").unwrap();
        writeln!(asm, "    ret").unwrap();

        let aligned_frame = ((st.max_temp_offset + 15) / 16) * 16;
        if aligned_frame > 0 {
            if aligned_frame <= 4095 {
                writeln!(prologue, "    sub sp, sp, #{}", aligned_frame).unwrap();
            } else {
                Self::emit_mov_imm(&mut prologue, "x9", aligned_frame as u64);
                writeln!(prologue, "    sub sp, sp, x9").unwrap();
            }
        }
        prologue.push_str(&asm);
        self.functions.push(prologue);
    }

    fn emit_bind_params(&mut self, asm: &mut String, st: &mut FnState, params: &[Param], env_arg_slot: i32) {
        let mut idx = 0usize;
        for p in params {
            if p.is_block {
                Self::emit_load_fp(asm, "x0", st.block_slot);
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
            } else if p.is_splat {
                let lbl_ok = self.new_label("splat_ok");
                let lbl_done = self.new_label("splat_done");
                Self::emit_load_fp(asm, "x0", st.argc_slot);
                Self::emit_mov_imm(asm, "x9", idx as u64);
                writeln!(asm, "    subs x0, x0, x9").unwrap();
                writeln!(asm, "    b.gt {}", lbl_ok).unwrap();
                writeln!(asm, "    mov x0, #0").unwrap();
                writeln!(asm, "    mov x1, #0").unwrap();
                writeln!(asm, "    bl rb_ary_new").unwrap();
                writeln!(asm, "    b {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_ok).unwrap();
                Self::emit_load_fp(asm, "x1", st.argv_slot);
                Self::emit_mov_imm(asm, "x9", (idx * 8) as u64);
                writeln!(asm, "    add x1, x1, x9").unwrap();
                writeln!(asm, "    bl rb_ary_new").unwrap();
                writeln!(asm, "{}:", lbl_done).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
            } else if let Some(ref def_expr) = p.default {
                let lbl_def = self.new_label("param_def");
                let lbl_done = self.new_label("param_done");
                Self::emit_load_fp(asm, "x9", st.argc_slot);
                Self::emit_mov_imm(asm, "x10", idx as u64);
                writeln!(asm, "    cmp x9, x10").unwrap();
                writeln!(asm, "    b.le {}", lbl_def).unwrap();
                Self::emit_load_fp(asm, "x9", st.argv_slot);
                Self::emit_mov_imm(asm, "x10", (idx * 8) as u64);
                writeln!(asm, "    ldr x0, [x9, x10]").unwrap();
                writeln!(asm, "    b {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_def).unwrap();
                self.emit_expr(asm, st, def_expr, env_arg_slot);
                writeln!(asm, "{}:", lbl_done).unwrap();
                self.emit_store_local(asm, st, &p.name, env_arg_slot);
                idx += 1;
            } else {
                let lbl_nil = self.new_label("param_nil");
                let lbl_done = self.new_label("param_done");
                Self::emit_load_fp(asm, "x9", st.argc_slot);
                Self::emit_mov_imm(asm, "x10", idx as u64);
                writeln!(asm, "    cmp x9, x10").unwrap();
                writeln!(asm, "    b.le {}", lbl_nil).unwrap();
                Self::emit_load_fp(asm, "x9", st.argv_slot);
                Self::emit_mov_imm(asm, "x10", (idx * 8) as u64);
                writeln!(asm, "    ldr x0, [x9, x10]").unwrap();
                writeln!(asm, "    b {}", lbl_done).unwrap();
                writeln!(asm, "{}:", lbl_nil).unwrap();
                writeln!(asm, "    mov x0, #4").unwrap();
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
            Self::emit_load_fp(asm, "x0", pslot);
        } else {
            let mut hops = depth;
            if cur_has_env {
                hops -= 1;
            }
            Self::emit_load_fp(asm, "x0", env_arg_slot);
            for _ in 0..hops {
                writeln!(asm, "    mov x1, #0").unwrap();
                writeln!(asm, "    bl rb_env_get").unwrap();
            }
        }
    }

    fn emit_load_local(&mut self, asm: &mut String, st: &FnState, name: &str, env_arg_slot: i32) -> bool {
        match self.lookup_var(st, name) {
            Some(VarLoc::Stack(off)) => {
                Self::emit_load_fp(asm, "x0", off);
                true
            }
            Some(VarLoc::Env(depth, idx)) => {
                Self::emit_load_env_ptr(asm, st, depth, env_arg_slot);
                Self::emit_mov_imm(asm, "x1", idx as u64);
                writeln!(asm, "    bl rb_env_get").unwrap();
                true
            }
            None => false,
        }
    }

    fn emit_store_local(&mut self, asm: &mut String, st: &mut FnState, name: &str, env_arg_slot: i32) {
        if let Some(loc) = self.lookup_var(st, name) {
            match loc {
                VarLoc::Stack(off) => {
                    Self::emit_store_fp(asm, "x0", off);
                }
                VarLoc::Env(depth, idx) => {
                    let t = st.alloc_temp();
                    Self::emit_store_fp(asm, "x0", t);
                    Self::emit_load_env_ptr(asm, st, depth, env_arg_slot);
                    Self::emit_mov_imm(asm, "x1", idx as u64);
                    Self::emit_load_fp(asm, "x2", t);
                    writeln!(asm, "    bl rb_env_set").unwrap();
                    Self::emit_load_fp(asm, "x0", t);
                    st.free_temp();
                }
            }
        }
    }

    fn emit_expr(&mut self, asm: &mut String, st: &mut FnState, expr: &Expr, env_arg_slot: i32) {
        match expr {
            Expr::Int(n) => {
                let tagged = (((*n as i64) << 1) | 1) as u64;
                Self::emit_mov_imm(asm, "x0", tagged);
            }
            Expr::Float(f) => {
                Self::emit_mov_imm(asm, "x0", f.to_bits());
                writeln!(asm, "    fmov d0, x0").unwrap();
                writeln!(asm, "    bl rb_float_new").unwrap();
            }
            Expr::Bool(true) => writeln!(asm, "    mov x0, #2").unwrap(),
            Expr::Bool(false) => writeln!(asm, "    mov x0, #0").unwrap(),
            Expr::Nil => writeln!(asm, "    mov x0, #4").unwrap(),
            Expr::SelfExpr => Self::emit_load_fp(asm, "x0", st.self_slot),

            Expr::Symbol(s) => {
                let lbl = self.intern_cstr(s);
                Self::emit_adr_sym(asm, "x0", &lbl);
                writeln!(asm, "    bl rb_sym_intern").unwrap();
            }

            Expr::StringLit(parts) => {
                if parts.is_empty() {
                    let lbl = self.intern_cstr("");
                    Self::emit_adr_sym(asm, "x0", &lbl);
                    writeln!(asm, "    mov x1, #0").unwrap();
                    writeln!(asm, "    bl rb_str_new").unwrap();
                } else if parts.len() == 1 {
                    match &parts[0] {
                        StringPart::Literal(s) => {
                            let lbl = self.intern_cstr(s);
                            Self::emit_adr_sym(asm, "x0", &lbl);
                            Self::emit_mov_imm(asm, "x1", s.len() as u64);
                            writeln!(asm, "    bl rb_str_new").unwrap();
                        }
                        StringPart::Interp(e) => {
                            self.emit_expr(asm, st, e, env_arg_slot);
                            writeln!(asm, "    bl rb_to_s").unwrap();
                        }
                    }
                } else {
                    let n = parts.len();
                    let base = st.alloc_temp_block(n);
                    for (i, p) in parts.iter().enumerate() {
                        match p {
                            StringPart::Literal(s) => {
                                let lbl = self.intern_cstr(s);
                                Self::emit_adr_sym(asm, "x0", &lbl);
                                Self::emit_mov_imm(asm, "x1", s.len() as u64);
                                writeln!(asm, "    bl rb_str_new").unwrap();
                            }
                            StringPart::Interp(e) => {
                                self.emit_expr(asm, st, e, env_arg_slot);
                            }
                        }
                        Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                    }
                    Self::emit_mov_imm(asm, "x0", n as u64);
                    Self::emit_addr_fp(asm, "x1", base);
                    writeln!(asm, "    bl rb_str_interp").unwrap();
                    st.free_temp_block(n);
                }
            }

            Expr::Array(items) => {
                let n = items.len();
                if n == 0 {
                    writeln!(asm, "    mov x0, #0").unwrap();
                    writeln!(asm, "    mov x1, #0").unwrap();
                    writeln!(asm, "    bl rb_ary_new").unwrap();
                } else {
                    let base = st.alloc_temp_block(n);
                    for (i, item) in items.iter().enumerate() {
                        self.emit_expr(asm, st, item, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                    }
                    Self::emit_mov_imm(asm, "x0", n as u64);
                    Self::emit_addr_fp(asm, "x1", base);
                    writeln!(asm, "    bl rb_ary_new").unwrap();
                    st.free_temp_block(n);
                }
            }

            Expr::Hash(pairs) => {
                let n = pairs.len();
                if n == 0 {
                    writeln!(asm, "    mov x0, #0").unwrap();
                    writeln!(asm, "    mov x1, #0").unwrap();
                    writeln!(asm, "    bl rb_hash_new").unwrap();
                } else {
                    let base = st.alloc_temp_block(n * 2);
                    for (i, (k, v)) in pairs.iter().enumerate() {
                        self.emit_expr(asm, st, k, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", base - (i as i32 * 2) * 8);
                        self.emit_expr(asm, st, v, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", base - (i as i32 * 2 + 1) * 8);
                    }
                    Self::emit_mov_imm(asm, "x0", n as u64);
                    Self::emit_addr_fp(asm, "x1", base);
                    writeln!(asm, "    bl rb_hash_new").unwrap();
                    st.free_temp_block(n * 2);
                }
            }

            Expr::Range { start, end, exclusive } => {
                let t = st.alloc_temp();
                self.emit_expr(asm, st, start, env_arg_slot);
                Self::emit_store_fp(asm, "x0", t);
                self.emit_expr(asm, st, end, env_arg_slot);
                writeln!(asm, "    mov x1, x0").unwrap();
                Self::emit_load_fp(asm, "x0", t);
                writeln!(asm, "    mov x2, #{}", if *exclusive { 1 } else { 0 }).unwrap();
                writeln!(asm, "    bl rb_range_new").unwrap();
                st.free_temp();
            }

            Expr::LocalVar(name) => {
                if !self.emit_load_local(asm, st, name, env_arg_slot) {
                    let lbl = self.intern_cstr(name);
                    Self::emit_load_fp(asm, "x0", st.self_slot);
                    Self::emit_adr_sym(asm, "x1", &lbl);
                    writeln!(asm, "    mov x2, #0").unwrap();
                    writeln!(asm, "    mov x3, #0").unwrap();
                    writeln!(asm, "    mov x4, #4").unwrap();
                    writeln!(asm, "    bl rb_send").unwrap();
                }
            }

            Expr::Ivar(name) => {
                let lbl = self.intern_cstr(name);
                Self::emit_load_fp(asm, "x0", st.self_slot);
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_ivar_get").unwrap();
            }

            Expr::Cvar(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_cvar_get").unwrap();
            }

            Expr::Gvar(name) => {
                let lbl = self.intern_cstr(name);
                Self::emit_adr_sym(asm, "x0", &lbl);
                writeln!(asm, "    bl rb_gvar_get").unwrap();
            }

            Expr::Const(name) => {
                let lbl = self.intern_cstr(name);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_const_get").unwrap();
            }

            Expr::ConstPath(parent, name) => {
                self.emit_expr(asm, st, parent, env_arg_slot);
                let lbl = self.intern_cstr(name);
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_const_get_path").unwrap();
            }

            Expr::Assign { target, op, value } => {
                if let Some(binop) = op {
                    if matches!(binop, BinOp::And | BinOp::Or) {
                        self.emit_load_target(asm, st, target, env_arg_slot);
                        let lbl_skip = self.new_label("short_assign");
                        let t = st.alloc_temp();
                        Self::emit_store_fp(asm, "x0", t);
                        writeln!(asm, "    bl rb_test").unwrap();
                        if matches!(binop, BinOp::Or) {
                            writeln!(asm, "    cbnz w0, {}", lbl_skip).unwrap();
                        } else {
                            writeln!(asm, "    cbz w0, {}", lbl_skip).unwrap();
                        }
                        self.emit_expr(asm, st, value, env_arg_slot);
                        self.emit_store_target(asm, st, target, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", t);
                        writeln!(asm, "{}:", lbl_skip).unwrap();
                        Self::emit_load_fp(asm, "x0", t);
                        st.free_temp();
                        return;
                    }
                    let t = st.alloc_temp();
                    self.emit_load_target(asm, st, target, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", t);
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
                    Self::emit_store_fp(asm, "x0", t);
                    for (i, tgt) in targets.iter().enumerate() {
                        Self::emit_load_fp(asm, "x0", t);
                        Self::emit_mov_imm(asm, "x1", i as u64);
                        writeln!(asm, "    bl rb_ary_get").unwrap();
                        self.emit_store_target(asm, st, tgt, env_arg_slot);
                    }
                    Self::emit_load_fp(asm, "x0", t);
                    st.free_temp();
                } else {
                    let n = values.len();
                    let base = st.alloc_temp_block(n);
                    for (i, v) in values.iter().enumerate() {
                        self.emit_expr(asm, st, v, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                    }
                    for (i, tgt) in targets.iter().enumerate() {
                        if i < n {
                            Self::emit_load_fp(asm, "x0", base - (i as i32) * 8);
                        } else {
                            writeln!(asm, "    mov x0, #4").unwrap();
                        }
                        self.emit_store_target(asm, st, tgt, env_arg_slot);
                    }
                    st.free_temp_block(n);
                }
            }

            Expr::BinOp { op, left, right } => match op {
                BinOp::And => {
                    let lbl_end = self.new_label("and_end");
                    self.emit_expr(asm, st, left, env_arg_slot);
                    let t = st.alloc_temp();
                    Self::emit_store_fp(asm, "x0", t);
                    writeln!(asm, "    bl rb_test").unwrap();
                    writeln!(asm, "    mov w9, w0").unwrap();
                    Self::emit_load_fp(asm, "x0", t);
                    writeln!(asm, "    cbz w9, {}", lbl_end).unwrap();
                    self.emit_expr(asm, st, right, env_arg_slot);
                    writeln!(asm, "{}:", lbl_end).unwrap();
                    st.free_temp();
                }
                BinOp::Or => {
                    let lbl_end = self.new_label("or_end");
                    self.emit_expr(asm, st, left, env_arg_slot);
                    let t = st.alloc_temp();
                    Self::emit_store_fp(asm, "x0", t);
                    writeln!(asm, "    bl rb_test").unwrap();
                    writeln!(asm, "    mov w9, w0").unwrap();
                    Self::emit_load_fp(asm, "x0", t);
                    writeln!(asm, "    cbnz w9, {}", lbl_end).unwrap();
                    self.emit_expr(asm, st, right, env_arg_slot);
                    writeln!(asm, "{}:", lbl_end).unwrap();
                    st.free_temp();
                }
                _ => {
                    let t = st.alloc_temp();
                    self.emit_expr(asm, st, left, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", t);
                    self.emit_expr(asm, st, right, env_arg_slot);
                    self.emit_binop_call(asm, op, t);
                    st.free_temp();
                }
            },

            Expr::UnaryOp { op, expr } => {
                self.emit_expr(asm, st, expr, env_arg_slot);
                match op {
                    UnaryOp::Neg => writeln!(asm, "    bl rb_neg").unwrap(),
                    UnaryOp::Pos => writeln!(asm, "    bl rb_pos").unwrap(),
                    UnaryOp::Not => writeln!(asm, "    bl rb_not").unwrap(),
                    UnaryOp::BitNot => writeln!(asm, "    bl rb_bitnot").unwrap(),
                }
            }

            Expr::If { cond, then_body, elsifs, else_body } => {
                let lbl_end = self.new_label("if_end");
                let mut lbl_next = self.new_label("if_next");

                self.emit_expr(asm, st, cond, env_arg_slot);
                writeln!(asm, "    bl rb_test").unwrap();
                writeln!(asm, "    cbz w0, {}", lbl_next).unwrap();

                self.emit_body(asm, st, then_body, env_arg_slot);
                writeln!(asm, "    b {}", lbl_end).unwrap();

                for (e_cond, e_body) in elsifs {
                    writeln!(asm, "{}:", lbl_next).unwrap();
                    lbl_next = self.new_label("elsif_next");
                    self.emit_expr(asm, st, e_cond, env_arg_slot);
                    writeln!(asm, "    bl rb_test").unwrap();
                    writeln!(asm, "    cbz w0, {}", lbl_next).unwrap();
                    self.emit_body(asm, st, e_body, env_arg_slot);
                    writeln!(asm, "    b {}", lbl_end).unwrap();
                }

                writeln!(asm, "{}:", lbl_next).unwrap();
                if let Some(eb) = else_body {
                    self.emit_body(asm, st, eb, env_arg_slot);
                } else {
                    writeln!(asm, "    mov x0, #4").unwrap();
                }
                writeln!(asm, "{}:", lbl_end).unwrap();
            }

            Expr::While { cond, body, is_until } => {
                let lbl_cond = self.new_label("while_cond");
                let lbl_end = self.new_label("while_end");
                st.loop_stack.push((lbl_cond.clone(), lbl_end.clone()));

                writeln!(asm, "{}:", lbl_cond).unwrap();
                self.emit_expr(asm, st, cond, env_arg_slot);
                writeln!(asm, "    bl rb_test").unwrap();
                if *is_until {
                    writeln!(asm, "    cbnz w0, {}", lbl_end).unwrap();
                } else {
                    writeln!(asm, "    cbz w0, {}", lbl_end).unwrap();
                }
                for s in body {
                    self.emit_expr(asm, st, s, env_arg_slot);
                }
                writeln!(asm, "    b {}", lbl_cond).unwrap();
                writeln!(asm, "{}:", lbl_end).unwrap();
                writeln!(asm, "    mov x0, #4").unwrap();
                st.loop_stack.pop();
            }

            Expr::For { var, iter, body } => {
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
                    Self::emit_store_fp(asm, "x0", t);
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
                            Self::emit_load_fp(asm, "x1", ts);
                            writeln!(asm, "    bl rb_case_eq").unwrap();
                        }
                        writeln!(asm, "    bl rb_test").unwrap();
                        writeln!(asm, "    cbnz w0, {}", lbl_body).unwrap();
                    }
                    writeln!(asm, "    b {}", lbl_next).unwrap();
                    writeln!(asm, "{}:", lbl_body).unwrap();
                    self.emit_body(asm, st, body, env_arg_slot);
                    writeln!(asm, "    b {}", lbl_end).unwrap();
                    writeln!(asm, "{}:", lbl_next).unwrap();
                }

                if let Some(eb) = else_body {
                    self.emit_body(asm, st, eb, env_arg_slot);
                } else {
                    writeln!(asm, "    mov x0, #4").unwrap();
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
                    writeln!(asm, "    mov x0, #4").unwrap();
                }
                if let Some((_, brk)) = st.loop_stack.last() {
                    writeln!(asm, "    b {}", brk).unwrap();
                } else {
                    writeln!(asm, "    b {}", st.epilogue_label).unwrap();
                }
            }

            Expr::Next(val) => {
                if let Some(v) = val {
                    self.emit_expr(asm, st, v, env_arg_slot);
                } else {
                    writeln!(asm, "    mov x0, #4").unwrap();
                }
                if let Some((cont, _)) = st.loop_stack.last() {
                    writeln!(asm, "    b {}", cont).unwrap();
                } else if st.is_block {
                    writeln!(asm, "    b {}", st.epilogue_label).unwrap();
                }
            }

            Expr::Return(val) => {
                if let Some(v) = val {
                    self.emit_expr(asm, st, v, env_arg_slot);
                } else {
                    writeln!(asm, "    mov x0, #4").unwrap();
                }
                writeln!(asm, "    b {}", st.epilogue_label).unwrap();
            }

            Expr::Yield(args) => {
                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, a, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                }
                Self::emit_load_fp(asm, "x0", st.block_slot);
                Self::emit_mov_imm(asm, "x1", n as u64);
                if n > 0 {
                    Self::emit_addr_fp(asm, "x2", base);
                } else {
                    writeln!(asm, "    mov x2, #0").unwrap();
                }
                writeln!(asm, "    mov x3, #4").unwrap();
                writeln!(asm, "    bl rb_proc_call").unwrap();
                if n > 0 {
                    st.free_temp_block(n);
                }
            }

            Expr::Super(args) => {
                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, &a.expr, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                }
                let mname = st.current_method_name.clone().unwrap_or_else(|| "initialize".to_string());
                let mlbl = self.intern_cstr(&mname);
                if let Some(dslot) = st.defining_class_slot {
                    Self::emit_load_fp(asm, "x0", dslot);
                } else {
                    writeln!(asm, "    mov x0, #0").unwrap();
                }
                Self::emit_load_fp(asm, "x1", st.self_slot);
                Self::emit_adr_sym(asm, "x2", &mlbl);
                Self::emit_mov_imm(asm, "x3", n as u64);
                if n > 0 {
                    Self::emit_addr_fp(asm, "x4", base);
                } else {
                    writeln!(asm, "    mov x4, #0").unwrap();
                }
                Self::emit_load_fp(asm, "x5", st.block_slot);
                writeln!(asm, "    bl rb_send_super").unwrap();
                if n > 0 {
                    st.free_temp_block(n);
                }
            }

            Expr::Call { receiver, name, args, block, safe_nav } => {
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
                    writeln!(asm, "    mov x0, #4").unwrap();
                    return;
                }
                if receiver.is_none() && name == "include" && args.len() == 1 {
                    self.emit_expr(asm, st, &args[0].expr, env_arg_slot);
                    writeln!(asm, "    mov x1, x0").unwrap();
                    Self::emit_load_fp(asm, "x0", st.self_slot);
                    writeln!(asm, "    bl rb_include_module").unwrap();
                    Self::emit_load_fp(asm, "x0", st.self_slot);
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
                    Self::emit_load_fp(asm, "x0", st.self_slot);
                }
                Self::emit_store_fp(asm, "x0", recv_slot);

                let lbl_nil_skip = if *safe_nav {
                    let l = self.new_label("safe_nav_nil");
                    writeln!(asm, "    cmp x0, #4").unwrap();
                    writeln!(asm, "    b.eq {}", l).unwrap();
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
                    Self::emit_adr_sym(asm, "x0", &blk_fn);
                    let cur_has_env = st.scopes.last().map(|s| s.has_env).unwrap_or(false);
                    if cur_has_env {
                        let pslot = st.scopes.last().unwrap().parent_env_slot.unwrap();
                        Self::emit_load_fp(asm, "x1", pslot);
                    } else {
                        Self::emit_load_fp(asm, "x1", env_arg_slot);
                    }
                    Self::emit_load_fp(asm, "x2", st.self_slot);
                    Self::emit_mov_imm(asm, "x3", blk.params.len() as u64);
                    writeln!(asm, "    bl rb_proc_new").unwrap();
                    Self::emit_store_fp(asm, "x0", blk_slot);
                } else {
                    writeln!(asm, "    mov x9, #4").unwrap();
                    Self::emit_store_fp(asm, "x9", blk_slot);
                }

                let n = args.len();
                let base = if n > 0 { st.alloc_temp_block(n) } else { 0 };
                for (i, a) in args.iter().enumerate() {
                    self.emit_expr(asm, st, &a.expr, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", base - (i as i32) * 8);
                }

                let mlbl = self.intern_cstr(name);
                Self::emit_load_fp(asm, "x0", recv_slot);
                Self::emit_adr_sym(asm, "x1", &mlbl);
                Self::emit_mov_imm(asm, "x2", n as u64);
                if n > 0 {
                    Self::emit_addr_fp(asm, "x3", base);
                } else {
                    writeln!(asm, "    mov x3, #0").unwrap();
                }
                Self::emit_load_fp(asm, "x4", blk_slot);
                writeln!(asm, "    bl rb_send").unwrap();

                if n > 0 {
                    st.free_temp_block(n);
                }
                st.free_temp();
                if let Some(l) = lbl_nil_skip {
                    writeln!(asm, "{}:", l).unwrap();
                }
                st.free_temp();
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
                Self::emit_adr_sym(asm, "x0", &lam_fn);
                let cur_has_env = st.scopes.last().map(|s| s.has_env).unwrap_or(false);
                if cur_has_env {
                    let pslot = st.scopes.last().unwrap().parent_env_slot.unwrap();
                    Self::emit_load_fp(asm, "x1", pslot);
                } else {
                    Self::emit_load_fp(asm, "x1", env_arg_slot);
                }
                Self::emit_load_fp(asm, "x2", st.self_slot);
                Self::emit_mov_imm(asm, "x3", params.len() as u64);
                writeln!(asm, "    bl rb_proc_new").unwrap();
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
                    Self::emit_adr_sym(asm, "x1", &mlbl);
                    Self::emit_adr_sym(asm, "x2", &fn_lbl);
                    Self::emit_mov_imm(asm, "x3", params.len() as u64);
                    writeln!(asm, "    bl rb_def_singleton_method").unwrap();
                } else {
                    let lbl_top = self.new_label("def_top");
                    let lbl_done = self.new_label("def_done");
                    Self::emit_load_fp(asm, "x0", st.self_slot);
                    writeln!(asm, "    ldr w9, [x0]").unwrap();
                    writeln!(asm, "    cmp w9, #6").unwrap();
                    writeln!(asm, "    b.ne {}", lbl_top).unwrap();
                    writeln!(asm, "    b {}", lbl_done).unwrap();
                    writeln!(asm, "{}:", lbl_top).unwrap();
                    writeln!(asm, "    bl rb_object_class").unwrap();
                    writeln!(asm, "{}:", lbl_done).unwrap();
                    Self::emit_adr_sym(asm, "x1", &mlbl);
                    Self::emit_adr_sym(asm, "x2", &fn_lbl);
                    Self::emit_mov_imm(asm, "x3", params.len() as u64);
                    writeln!(asm, "    bl rb_def_method").unwrap();
                }
                Self::emit_adr_sym(asm, "x0", &mlbl);
                writeln!(asm, "    bl rb_sym_intern").unwrap();
            }

            Expr::Class { name, superclass, body } => {
                let super_slot = st.alloc_temp();
                if let Some(sup) = superclass {
                    self.emit_expr(asm, st, sup, env_arg_slot);
                    Self::emit_store_fp(asm, "x0", super_slot);
                } else {
                    writeln!(asm, "    mov x9, #0").unwrap();
                    Self::emit_store_fp(asm, "x9", super_slot);
                }
                let clbl = self.intern_cstr(name);
                Self::emit_adr_sym(asm, "x0", &clbl);
                Self::emit_load_fp(asm, "x1", super_slot);
                writeln!(asm, "    mov x2, #0").unwrap();
                writeln!(asm, "    bl rb_define_class").unwrap();
                st.free_temp();

                let cls_slot = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", cls_slot);

                let body_fn = self.new_fn_label(&format!("class_{}", name));
                self.compile_function(&body_fn, &[], body, None, None, false, vec![]);

                writeln!(asm, "    mov x0, #0").unwrap();
                Self::emit_load_fp(asm, "x1", cls_slot);
                writeln!(asm, "    mov x2, #0").unwrap();
                writeln!(asm, "    mov x3, #0").unwrap();
                writeln!(asm, "    mov x4, #4").unwrap();
                writeln!(asm, "    bl {}", body_fn).unwrap();
                st.free_temp();
            }

            Expr::Module { name, body } => {
                let mlbl = self.intern_cstr(name);
                Self::emit_adr_sym(asm, "x0", &mlbl);
                writeln!(asm, "    mov x1, #0").unwrap();
                writeln!(asm, "    bl rb_define_module").unwrap();

                let mod_slot = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", mod_slot);

                let body_fn = self.new_fn_label(&format!("mod_{}", name));
                self.compile_function(&body_fn, &[], body, None, None, false, vec![]);

                writeln!(asm, "    mov x0, #0").unwrap();
                Self::emit_load_fp(asm, "x1", mod_slot);
                writeln!(asm, "    mov x2, #0").unwrap();
                writeln!(asm, "    mov x3, #0").unwrap();
                writeln!(asm, "    mov x4, #4").unwrap();
                writeln!(asm, "    bl {}", body_fn).unwrap();
                st.free_temp();
            }

            Expr::Begin { body, rescues, ensure } => {
                if rescues.is_empty() && ensure.is_none() {
                    self.emit_body(asm, st, body, env_arg_slot);
                    return;
                }
                let jmp_buf_off = st.alloc_temp_block(32);
                let res_slot = st.alloc_temp();
                let lbl_rescue = self.new_label("rescue");
                let lbl_ensure = self.new_label("ensure");

                Self::emit_addr_fp(asm, "x0", jmp_buf_off);
                writeln!(asm, "    bl rb_push_rescue").unwrap();
                Self::emit_addr_fp(asm, "x0", jmp_buf_off);
                writeln!(asm, "    bl _setjmp").unwrap();
                writeln!(asm, "    cbnz w0, {}", lbl_rescue).unwrap();

                self.emit_body(asm, st, body, env_arg_slot);
                Self::emit_store_fp(asm, "x0", res_slot);
                writeln!(asm, "    bl rb_pop_rescue").unwrap();
                writeln!(asm, "    b {}", lbl_ensure).unwrap();

                writeln!(asm, "{}:", lbl_rescue).unwrap();
                if rescues.is_empty() {
                    if let Some(ens) = ensure {
                        self.emit_body(asm, st, ens, env_arg_slot);
                    }
                    writeln!(asm, "    bl rb_current_exception").unwrap();
                    writeln!(asm, "    bl rb_raise").unwrap();
                } else {
                    let exc_slot = st.alloc_temp();
                    writeln!(asm, "    bl rb_current_exception").unwrap();
                    Self::emit_store_fp(asm, "x0", exc_slot);
                    for r in rescues {
                        let lbl_match = self.new_label("rescue_match");
                        let lbl_next = self.new_label("rescue_next");
                        if r.classes.is_empty() {
                            writeln!(asm, "    b {}", lbl_match).unwrap();
                        } else {
                            for cls_expr in &r.classes {
                                self.emit_expr(asm, st, cls_expr, env_arg_slot);
                                writeln!(asm, "    mov x1, x0").unwrap();
                                Self::emit_load_fp(asm, "x0", exc_slot);
                                writeln!(asm, "    bl rb_exception_matches").unwrap();
                                writeln!(asm, "    cbnz w0, {}", lbl_match).unwrap();
                            }
                            writeln!(asm, "    b {}", lbl_next).unwrap();
                        }
                        writeln!(asm, "{}:", lbl_match).unwrap();
                        if let Some(v) = &r.var {
                            Self::emit_load_fp(asm, "x0", exc_slot);
                            self.emit_store_local(asm, st, v, env_arg_slot);
                        }
                        self.emit_body(asm, st, &r.body, env_arg_slot);
                        Self::emit_store_fp(asm, "x0", res_slot);
                        writeln!(asm, "    b {}", lbl_ensure).unwrap();
                        writeln!(asm, "{}:", lbl_next).unwrap();
                    }
                    if let Some(ens) = ensure {
                        self.emit_body(asm, st, ens, env_arg_slot);
                    }
                    Self::emit_load_fp(asm, "x0", exc_slot);
                    writeln!(asm, "    bl rb_raise").unwrap();
                    st.free_temp();
                }

                writeln!(asm, "{}:", lbl_ensure).unwrap();
                if let Some(ens) = ensure {
                    self.emit_body(asm, st, ens, env_arg_slot);
                }
                Self::emit_load_fp(asm, "x0", res_slot);
                st.free_temp();
                st.free_temp_block(32);
            }
        }
    }

    fn emit_body(&mut self, asm: &mut String, st: &mut FnState, body: &[Expr], env_arg_slot: i32) {
        if body.is_empty() {
            writeln!(asm, "    mov x0, #4").unwrap();
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
                    writeln!(asm, "    mov x0, #4").unwrap();
                }
            }
            AssignTarget::Ivar(n) => {
                let lbl = self.intern_cstr(n);
                Self::emit_load_fp(asm, "x0", st.self_slot);
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_ivar_get").unwrap();
            }
            AssignTarget::Cvar(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_cvar_get").unwrap();
            }
            AssignTarget::Gvar(n) => {
                let lbl = self.intern_cstr(n);
                Self::emit_adr_sym(asm, "x0", &lbl);
                writeln!(asm, "    bl rb_gvar_get").unwrap();
            }
            AssignTarget::Const(n) => {
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                writeln!(asm, "    bl rb_const_get").unwrap();
            }
        }
    }

    fn emit_store_target(&mut self, asm: &mut String, st: &mut FnState, target: &AssignTarget, env_arg_slot: i32) {
        match target {
            AssignTarget::Local(n) => self.emit_store_local(asm, st, n, env_arg_slot),
            AssignTarget::Ivar(n) => {
                let t = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", t);
                let lbl = self.intern_cstr(n);
                Self::emit_load_fp(asm, "x0", st.self_slot);
                Self::emit_adr_sym(asm, "x1", &lbl);
                Self::emit_load_fp(asm, "x2", t);
                writeln!(asm, "    bl rb_ivar_set").unwrap();
                st.free_temp();
            }
            AssignTarget::Cvar(n) => {
                let t = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", t);
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                Self::emit_load_fp(asm, "x2", t);
                writeln!(asm, "    bl rb_cvar_set").unwrap();
                st.free_temp();
            }
            AssignTarget::Gvar(n) => {
                let t = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", t);
                let lbl = self.intern_cstr(n);
                Self::emit_adr_sym(asm, "x0", &lbl);
                Self::emit_load_fp(asm, "x1", t);
                writeln!(asm, "    bl rb_gvar_set").unwrap();
                st.free_temp();
            }
            AssignTarget::Const(n) => {
                let t = st.alloc_temp();
                Self::emit_store_fp(asm, "x0", t);
                let lbl = self.intern_cstr(n);
                writeln!(asm, "    bl rb_object_class").unwrap();
                Self::emit_adr_sym(asm, "x1", &lbl);
                Self::emit_load_fp(asm, "x2", t);
                writeln!(asm, "    bl rb_const_set").unwrap();
                st.free_temp();
            }
        }
    }

    fn emit_binop_call(&self, asm: &mut String, op: &BinOp, left_slot: i32) {
        writeln!(asm, "    mov x1, x0").unwrap();
        Self::emit_load_fp(asm, "x0", left_slot);
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
        writeln!(asm, "    bl {}", func).unwrap();
    }
}
