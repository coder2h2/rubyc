#ifndef RUBYCOM_RUNTIME_H
#define RUBYCOM_RUNTIME_H

#include <stdint.h>
#include <stddef.h>
#include <setjmp.h>

typedef uint64_t RValue;

#define QFALSE ((RValue)0x00)
#define QTRUE  ((RValue)0x02)
#define QNIL   ((RValue)0x04)
#define QUNDEF ((RValue)0x06)

#define RUBY_FIXNUM_FLAG 0x01
#define RUBY_SYMBOL_TAG  0x0C

static inline int rb_is_fixnum(RValue v) {
    return (v & 1) == 1;
}

static inline int64_t rb_fixnum_val(RValue v) {
    return ((int64_t)v) >> 1;
}

static inline RValue rb_fixnum(int64_t n) {
    return (RValue)((n << 1) | 1);
}

static inline int rb_is_symbol(RValue v) {
    return (v & 0xFF) == RUBY_SYMBOL_TAG;
}

static inline uint32_t rb_symbol_id(RValue v) {
    return (uint32_t)(v >> 8);
}

static inline RValue rb_make_symbol(uint32_t id) {
    return (((RValue)id) << 8) | RUBY_SYMBOL_TAG;
}

static inline int rb_is_ptr(RValue v) {
    return (v > 6) && ((v & 7) == 0);
}

static inline int rb_is_truthy(RValue v) {
    return v != QFALSE && v != QNIL;
}

typedef enum {
    OBJ_FLOAT = 1,
    OBJ_STRING,
    OBJ_ARRAY,
    OBJ_HASH,
    OBJ_RANGE,
    OBJ_CLASS,
    OBJ_INSTANCE,
    OBJ_PROC,
    OBJ_EXCEPTION,
    OBJ_ENV
} RObjType;

typedef RValue (*RbFn)(RValue env, RValue self, int64_t argc, RValue* argv, RValue block);

typedef struct RObj {
    RObjType type;
    uint8_t marked;
    struct RObj* gc_next;
} RObj;

typedef struct RFloat {
    RObj base;
    double val;
} RFloat;

typedef struct RString {
    RObj base;
    char* data;
    size_t len;
    size_t cap;
} RString;

typedef struct RArray {
    RObj base;
    RValue* items;
    size_t len;
    size_t cap;
} RArray;

typedef struct RHashEntry {
    RValue key;
    RValue val;
} RHashEntry;

typedef struct RHash {
    RObj base;
    RHashEntry* entries;
    size_t len;
    size_t cap;
} RHash;

typedef struct RRange {
    RObj base;
    RValue start;
    RValue end;
    int exclusive;
} RRange;

typedef struct RMethodEntry {
    const char* name;
    RbFn fn;
    int arity;
} RMethodEntry;

typedef struct RKVPair {
    const char* key;
    RValue val;
} RKVPair;

typedef struct RClass {
    RObj base;
    const char* name;
    struct RClass* superclass;
    int is_module;
    RMethodEntry* methods;
    size_t num_methods;
    size_t cap_methods;
    RKVPair* ivars;
    size_t num_ivars;
    size_t cap_ivars;
    RKVPair* cvars;
    size_t num_cvars;
    size_t cap_cvars;
    RKVPair* consts;
    size_t num_consts;
    size_t cap_consts;
    struct RClass** includes;
    size_t num_includes;
    size_t cap_includes;
} RClass;

typedef struct RInstance {
    RObj base;
    RClass* klass;
    RKVPair* ivars;
    size_t num_ivars;
    size_t cap_ivars;
} RInstance;

typedef struct RProc {
    RObj base;
    RbFn fn;
    RValue env;
    RValue captured_self;
    int arity;
} RProc;

typedef struct RException {
    RObj base;
    RClass* klass;
    RValue message;
} RException;

typedef struct REnv {
    RObj base;
    RValue* slots;
    size_t size;
} REnv;

/* Initialization & Lifecycle */
void rb_init(int argc, char** argv);
void rb_shutdown(void);
RValue rb_top_self(void);
RClass* rb_object_class(void);

/* Constructors */
RValue rb_float_new(double d);
RValue rb_str_new(const char* s, size_t len);
RValue rb_str_new_cstr(const char* s);
RValue rb_str_concat(RValue a, RValue b);
RValue rb_str_interp(int64_t count, RValue* parts);
RValue rb_sym_intern(const char* name);
RValue rb_ary_new(int64_t argc, RValue* argv);
RValue rb_ary_push(RValue ary, RValue val);
RValue rb_ary_get(RValue ary, int64_t idx);
RValue rb_hash_new(int64_t pair_count, RValue* kvs);
RValue rb_range_new(RValue start, RValue end, int exclusive);
RValue rb_proc_new(RbFn fn, RValue env, RValue captured_self, int arity);
RValue rb_env_alloc(size_t size);
RValue rb_env_get(RValue env, size_t idx);
void rb_env_set(RValue env, size_t idx, RValue val);

/* Classes, Modules, Methods, Variables */
RClass* rb_define_class(const char* name, RClass* superclass, RClass* outer);
RClass* rb_define_module(const char* name, RClass* outer);
RClass* rb_get_class(const char* name);
void rb_def_method(RClass* klass, const char* name, RbFn fn, int arity);
void rb_def_singleton_method(RValue obj, const char* name, RbFn fn, int arity);
void rb_include_module(RClass* klass, RClass* mod);

RValue rb_ivar_get(RValue obj, const char* name);
RValue rb_ivar_set(RValue obj, const char* name, RValue val);
RValue rb_cvar_get(RClass* klass, const char* name);
RValue rb_cvar_set(RClass* klass, const char* name, RValue val);
RValue rb_gvar_get(const char* name);
RValue rb_gvar_set(const char* name, RValue val);
RValue rb_const_get(RClass* scope, const char* name);
RValue rb_const_set(RClass* scope, const char* name, RValue val);
RValue rb_const_get_path(RValue parent, const char* name);

/* Dispatch & Operators */
RValue rb_send(RValue recv, const char* name, int64_t argc, RValue* argv, RValue block);
RValue rb_send_super(RClass* defining_klass, RValue recv, const char* name, int64_t argc, RValue* argv, RValue block);
RValue rb_proc_call(RValue proc, int64_t argc, RValue* argv, RValue block);

RValue rb_add(RValue a, RValue b);
RValue rb_sub(RValue a, RValue b);
RValue rb_mul(RValue a, RValue b);
RValue rb_div(RValue a, RValue b);
RValue rb_mod(RValue a, RValue b);
RValue rb_pow(RValue a, RValue b);
RValue rb_eq(RValue a, RValue b);
RValue rb_neq(RValue a, RValue b);
RValue rb_lt(RValue a, RValue b);
RValue rb_le(RValue a, RValue b);
RValue rb_gt(RValue a, RValue b);
RValue rb_ge(RValue a, RValue b);
RValue rb_cmp(RValue a, RValue b);
RValue rb_case_eq(RValue pattern, RValue target);
RValue rb_neg(RValue a);
RValue rb_pos(RValue a);
RValue rb_not(RValue a);
RValue rb_bitnot(RValue a);
RValue rb_bitand(RValue a, RValue b);
RValue rb_bitor(RValue a, RValue b);
RValue rb_bitxor(RValue a, RValue b);
RValue rb_shl(RValue a, RValue b);
RValue rb_shr(RValue a, RValue b);
int rb_test(RValue v);

/* Exceptions */
void rb_push_rescue(jmp_buf* buf);
void rb_pop_rescue(void);
void rb_raise(RValue exc);
void rb_raise_msg(RClass* klass, const char* msg);
RValue rb_current_exception(void);
int rb_exception_matches(RValue exc, RValue klass_val);

/* Helpers */
RValue rb_to_s(RValue v);
RValue rb_inspect(RValue v);
void rb_check_args(int64_t argc, int min_args, int max_args, const char* name);

#endif
