#define _POSIX_C_SOURCE 200809L
#include "runtime.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <ctype.h>
#include <time.h>
#include <unistd.h>
#include <stdarg.h>

/* =========================================================================
 * Global State & Built-in Classes
 * ========================================================================= */

static RObj* g_all_objs = NULL;
static size_t g_num_objs = 0;
static size_t g_gc_threshold = 250000;
static uintptr_t g_stack_bottom = 0;

static RClass* cBasicObject = NULL;
static RClass* cObject = NULL;
static RClass* cModule = NULL;
static RClass* cClass = NULL;
static RClass* cInteger = NULL;
static RClass* cFloat = NULL;
static RClass* cString = NULL;
static RClass* cArray = NULL;
static RClass* cHash = NULL;
static RClass* cRange = NULL;
static RClass* cSymbol = NULL;
static RClass* cTrueClass = NULL;
static RClass* cFalseClass = NULL;
static RClass* cNilClass = NULL;
static RClass* cProc = NULL;
static RClass* cException = NULL;
static RClass* cStandardError = NULL;
static RClass* cRuntimeError = NULL;
static RClass* cTypeError = NULL;
static RClass* cArgumentError = NULL;
static RClass* cNoMethodError = NULL;
static RClass* cZeroDivisionError = NULL;
static RClass* cIndexError = NULL;
static RClass* cNameError = NULL;
static RClass* mKernel = NULL;
static RClass* mMath = NULL;
static RClass* cFile = NULL;

static RValue g_top_self = QNIL;
static RValue g_current_exc = QNIL;

static char** g_sym_names = NULL;
static size_t g_num_syms = 0;
static size_t g_cap_syms = 0;

static RKVPair* g_gvars = NULL;
static size_t g_num_gvars = 0;
static size_t g_cap_gvars = 0;

typedef struct SingletonEntry {
    RValue obj;
    const char* name;
    RbFn fn;
    int arity;
} SingletonEntry;

static SingletonEntry* g_singletons = NULL;
static size_t g_num_singletons = 0;
static size_t g_cap_singletons = 0;

#define MAX_RESCUE_DEPTH 256
static jmp_buf* g_rescue_stack[MAX_RESCUE_DEPTH];
static int g_rescue_top = 0;

/* =========================================================================
 * Allocation & Garbage Collection
 * ========================================================================= */

static void gc_mark_val(RValue v);

static void gc_mark_obj(RObj* obj) {
    if (!obj || obj->marked) return;
    obj->marked = 1;
    switch (obj->type) {
        case OBJ_FLOAT:
        case OBJ_STRING:
            break;
        case OBJ_ARRAY: {
            RArray* a = (RArray*)obj;
            for (size_t i = 0; i < a->len; i++) gc_mark_val(a->items[i]);
            break;
        }
        case OBJ_HASH: {
            RHash* h = (RHash*)obj;
            for (size_t i = 0; i < h->len; i++) {
                gc_mark_val(h->entries[i].key);
                gc_mark_val(h->entries[i].val);
            }
            break;
        }
        case OBJ_RANGE: {
            RRange* r = (RRange*)obj;
            gc_mark_val(r->start);
            gc_mark_val(r->end);
            break;
        }
        case OBJ_CLASS: {
            RClass* c = (RClass*)obj;
            if (c->superclass) gc_mark_obj((RObj*)c->superclass);
            for (size_t i = 0; i < c->num_ivars; i++) gc_mark_val(c->ivars[i].val);
            for (size_t i = 0; i < c->num_cvars; i++) gc_mark_val(c->cvars[i].val);
            for (size_t i = 0; i < c->num_consts; i++) gc_mark_val(c->consts[i].val);
            for (size_t i = 0; i < c->num_includes; i++) gc_mark_obj((RObj*)c->includes[i]);
            break;
        }
        case OBJ_INSTANCE: {
            RInstance* inst = (RInstance*)obj;
            if (inst->klass) gc_mark_obj((RObj*)inst->klass);
            for (size_t i = 0; i < inst->num_ivars; i++) gc_mark_val(inst->ivars[i].val);
            break;
        }
        case OBJ_PROC: {
            RProc* p = (RProc*)obj;
            gc_mark_val(p->env);
            gc_mark_val(p->captured_self);
            break;
        }
        case OBJ_EXCEPTION: {
            RException* e = (RException*)obj;
            if (e->klass) gc_mark_obj((RObj*)e->klass);
            gc_mark_val(e->message);
            break;
        }
        case OBJ_ENV: {
            REnv* env = (REnv*)obj;
            for (size_t i = 0; i < env->size; i++) gc_mark_val(env->slots[i]);
            break;
        }
    }
}

static void gc_mark_val(RValue v) {
    if (rb_is_ptr(v)) {
        gc_mark_obj((RObj*)(uintptr_t)v);
    }
}

static void rb_gc_run(void) {
    /* Mark permanent roots */
    gc_mark_obj((RObj*)cBasicObject);
    gc_mark_obj((RObj*)cObject);
    gc_mark_val(g_top_self);
    gc_mark_val(g_current_exc);
    for (size_t i = 0; i < g_num_gvars; i++) gc_mark_val(g_gvars[i].val);
    for (size_t i = 0; i < g_num_singletons; i++) gc_mark_val(g_singletons[i].obj);

    /* Conservative stack scan */
    volatile uintptr_t sp_marker = 0;
    uintptr_t top = (uintptr_t)&sp_marker;
    if (g_stack_bottom > top) {
        for (uintptr_t p = top; p < g_stack_bottom; p += sizeof(uintptr_t)) {
            RValue candidate = *(RValue*)p;
            if (rb_is_ptr(candidate)) {
                /* Verify candidate is in g_all_objs before marking */
                RObj* cur = g_all_objs;
                while (cur) {
                    if ((RValue)(uintptr_t)cur == candidate) {
                        gc_mark_obj(cur);
                        break;
                    }
                    cur = cur->gc_next;
                }
            }
        }
    }

    /* Sweep */
    RObj** prev = &g_all_objs;
    while (*prev) {
        RObj* obj = *prev;
        if (obj->marked) {
            obj->marked = 0;
            prev = &obj->gc_next;
        } else {
            *prev = obj->gc_next;
            g_num_objs--;
            switch (obj->type) {
                case OBJ_STRING:
                    free(((RString*)obj)->data);
                    break;
                case OBJ_ARRAY:
                    free(((RArray*)obj)->items);
                    break;
                case OBJ_HASH:
                    free(((RHash*)obj)->entries);
                    break;
                case OBJ_CLASS: {
                    RClass* c = (RClass*)obj;
                    free(c->methods);
                    free(c->ivars);
                    free(c->cvars);
                    free(c->consts);
                    free(c->includes);
                    break;
                }
                case OBJ_INSTANCE:
                    free(((RInstance*)obj)->ivars);
                    break;
                case OBJ_ENV:
                    free(((REnv*)obj)->slots);
                    break;
                default:
                    break;
            }
            free(obj);
        }
    }
    if (g_num_objs * 2 > g_gc_threshold) {
        g_gc_threshold = g_num_objs * 2;
    }
}

static RObj* rb_alloc_obj(RObjType type, size_t size) {
    if (g_num_objs >= g_gc_threshold) {
        rb_gc_run();
    }
    RObj* obj = (RObj*)calloc(1, size);
    if (!obj) {
        fprintf(stderr, "rubycom: out of memory\n");
        exit(1);
    }
    obj->type = type;
    obj->marked = 0;
    obj->gc_next = g_all_objs;
    g_all_objs = obj;
    g_num_objs++;
    return obj;
}

/* =========================================================================
 * Constructors
 * ========================================================================= */

RValue rb_float_new(double d) {
    RFloat* f = (RFloat*)rb_alloc_obj(OBJ_FLOAT, sizeof(RFloat));
    f->val = d;
    return (RValue)(uintptr_t)f;
}

RValue rb_str_new(const char* s, size_t len) {
    RString* str = (RString*)rb_alloc_obj(OBJ_STRING, sizeof(RString));
    str->len = len;
    str->cap = len + 1;
    str->data = (char*)malloc(str->cap);
    if (len > 0 && s) memcpy(str->data, s, len);
    str->data[len] = '\0';
    return (RValue)(uintptr_t)str;
}

RValue rb_str_new_cstr(const char* s) {
    return rb_str_new(s, s ? strlen(s) : 0);
}

RValue rb_str_concat(RValue a, RValue b) {
    RValue sa = rb_to_s(a);
    RValue sb = rb_to_s(b);
    RString* A = (RString*)(uintptr_t)sa;
    RString* B = (RString*)(uintptr_t)sb;
    size_t nlen = A->len + B->len;
    RString* out = (RString*)rb_alloc_obj(OBJ_STRING, sizeof(RString));
    out->len = nlen;
    out->cap = nlen + 1;
    out->data = (char*)malloc(out->cap);
    memcpy(out->data, A->data, A->len);
    memcpy(out->data + A->len, B->data, B->len);
    out->data[nlen] = '\0';
    return (RValue)(uintptr_t)out;
}

RValue rb_str_interp(int64_t count, RValue* parts) {
    size_t total = 0;
    RValue* strs = (RValue*)malloc(sizeof(RValue) * (size_t)(count > 0 ? count : 1));
    for (int64_t i = 0; i < count; i++) {
        strs[i] = rb_to_s(parts[i]);
        total += ((RString*)(uintptr_t)strs[i])->len;
    }
    RString* out = (RString*)rb_alloc_obj(OBJ_STRING, sizeof(RString));
    out->len = total;
    out->cap = total + 1;
    out->data = (char*)malloc(out->cap);
    size_t pos = 0;
    for (int64_t i = 0; i < count; i++) {
        RString* s = (RString*)(uintptr_t)strs[i];
        memcpy(out->data + pos, s->data, s->len);
        pos += s->len;
    }
    out->data[total] = '\0';
    free(strs);
    return (RValue)(uintptr_t)out;
}

RValue rb_sym_intern(const char* name) {
    for (size_t i = 0; i < g_num_syms; i++) {
        if (strcmp(g_sym_names[i], name) == 0) {
            return rb_make_symbol((uint32_t)i);
        }
    }
    if (g_num_syms >= g_cap_syms) {
        g_cap_syms = g_cap_syms ? g_cap_syms * 2 : 64;
        g_sym_names = (char**)realloc(g_sym_names, sizeof(char*) * g_cap_syms);
    }
    uint32_t id = (uint32_t)g_num_syms++;
    g_sym_names[id] = strdup(name);
    return rb_make_symbol(id);
}

static const char* rb_sym_name(RValue sym) {
    uint32_t id = rb_symbol_id(sym);
    if (id < g_num_syms) return g_sym_names[id];
    return "";
}

RValue rb_ary_new(int64_t argc, RValue* argv) {
    RArray* a = (RArray*)rb_alloc_obj(OBJ_ARRAY, sizeof(RArray));
    a->len = (size_t)(argc > 0 ? argc : 0);
    a->cap = a->len > 4 ? a->len : 4;
    a->items = (RValue*)malloc(sizeof(RValue) * a->cap);
    for (size_t i = 0; i < a->len; i++) a->items[i] = argv[i];
    return (RValue)(uintptr_t)a;
}

RValue rb_ary_push(RValue ary, RValue val) {
    RArray* a = (RArray*)(uintptr_t)ary;
    if (a->len >= a->cap) {
        a->cap = a->cap ? a->cap * 2 : 4;
        a->items = (RValue*)realloc(a->items, sizeof(RValue) * a->cap);
    }
    a->items[a->len++] = val;
    return ary;
}

RValue rb_ary_get(RValue ary, int64_t idx) {
    if (!rb_is_ptr(ary) || ((RObj*)(uintptr_t)ary)->type != OBJ_ARRAY) return QNIL;
    RArray* a = (RArray*)(uintptr_t)ary;
    if (idx < 0) idx += (int64_t)a->len;
    if (idx < 0 || (size_t)idx >= a->len) return QNIL;
    return a->items[idx];
}

RValue rb_hash_new(int64_t pair_count, RValue* kvs) {
    RHash* h = (RHash*)rb_alloc_obj(OBJ_HASH, sizeof(RHash));
    h->cap = pair_count > 4 ? (size_t)pair_count : 4;
    h->len = 0;
    h->entries = (RHashEntry*)malloc(sizeof(RHashEntry) * h->cap);
    for (int64_t i = 0; i < pair_count; i++) {
        RValue k = kvs[i * 2];
        RValue v = kvs[i * 2 + 1];
        int found = 0;
        for (size_t j = 0; j < h->len; j++) {
            if (rb_test(rb_eq(h->entries[j].key, k))) {
                h->entries[j].val = v;
                found = 1;
                break;
            }
        }
        if (!found) {
            if (h->len >= h->cap) {
                h->cap *= 2;
                h->entries = (RHashEntry*)realloc(h->entries, sizeof(RHashEntry) * h->cap);
            }
            h->entries[h->len].key = k;
            h->entries[h->len].val = v;
            h->len++;
        }
    }
    return (RValue)(uintptr_t)h;
}

RValue rb_range_new(RValue start, RValue end, int exclusive) {
    RRange* r = (RRange*)rb_alloc_obj(OBJ_RANGE, sizeof(RRange));
    r->start = start;
    r->end = end;
    r->exclusive = exclusive;
    return (RValue)(uintptr_t)r;
}

RValue rb_proc_new(RbFn fn, RValue env, RValue captured_self, int arity) {
    RProc* p = (RProc*)rb_alloc_obj(OBJ_PROC, sizeof(RProc));
    p->fn = fn;
    p->env = env;
    p->captured_self = captured_self;
    p->arity = arity;
    return (RValue)(uintptr_t)p;
}

RValue rb_env_alloc(size_t size) {
    REnv* env = (REnv*)rb_alloc_obj(OBJ_ENV, sizeof(REnv));
    env->size = size;
    env->slots = (RValue*)malloc(sizeof(RValue) * (size > 0 ? size : 1));
    for (size_t i = 0; i < size; i++) env->slots[i] = QNIL;
    return (RValue)(uintptr_t)env;
}

RValue rb_env_get(RValue env_val, size_t idx) {
    REnv* env = (REnv*)(uintptr_t)env_val;
    return env->slots[idx];
}

void rb_env_set(RValue env_val, size_t idx, RValue val) {
    REnv* env = (REnv*)(uintptr_t)env_val;
    env->slots[idx] = val;
}

/* =========================================================================
 * Class / Module / Method / Variable Management
 * ========================================================================= */

static RClass* rb_class_of(RValue v) {
    if (v == QFALSE) return cFalseClass;
    if (v == QTRUE) return cTrueClass;
    if (v == QNIL) return cNilClass;
    if (rb_is_fixnum(v)) return cInteger;
    if (rb_is_symbol(v)) return cSymbol;
    if (rb_is_ptr(v)) {
        RObj* obj = (RObj*)(uintptr_t)v;
        switch (obj->type) {
            case OBJ_FLOAT: return cFloat;
            case OBJ_STRING: return cString;
            case OBJ_ARRAY: return cArray;
            case OBJ_HASH: return cHash;
            case OBJ_RANGE: return cRange;
            case OBJ_PROC: return cProc;
            case OBJ_CLASS: return ((RClass*)obj)->is_module ? cModule : cClass;
            case OBJ_INSTANCE: return ((RInstance*)obj)->klass;
            case OBJ_EXCEPTION: return ((RException*)obj)->klass;
            default: return cObject;
        }
    }
    return cObject;
}

RClass* rb_object_class(void) {
    return cObject;
}

RValue rb_top_self(void) {
    return g_top_self;
}

RClass* rb_define_class(const char* name, RClass* superclass, RClass* outer) {
    RClass* scope = outer ? outer : cObject;
    /* Check if already defined in scope */
    if (scope) {
        for (size_t i = 0; i < scope->num_consts; i++) {
            if (strcmp(scope->consts[i].key, name) == 0 && rb_is_ptr(scope->consts[i].val)) {
                RObj* obj = (RObj*)(uintptr_t)scope->consts[i].val;
                if (obj->type == OBJ_CLASS) return (RClass*)obj;
            }
        }
    }
    RClass* c = (RClass*)rb_alloc_obj(OBJ_CLASS, sizeof(RClass));
    c->name = strdup(name);
    c->superclass = superclass ? superclass : cObject;
    c->is_module = 0;
    if (scope) {
        rb_const_set(scope, name, (RValue)(uintptr_t)c);
    }
    return c;
}

RClass* rb_define_module(const char* name, RClass* outer) {
    RClass* scope = outer ? outer : cObject;
    if (scope) {
        for (size_t i = 0; i < scope->num_consts; i++) {
            if (strcmp(scope->consts[i].key, name) == 0 && rb_is_ptr(scope->consts[i].val)) {
                RObj* obj = (RObj*)(uintptr_t)scope->consts[i].val;
                if (obj->type == OBJ_CLASS) return (RClass*)obj;
            }
        }
    }
    RClass* m = (RClass*)rb_alloc_obj(OBJ_CLASS, sizeof(RClass));
    m->name = strdup(name);
    m->superclass = NULL;
    m->is_module = 1;
    if (scope) {
        rb_const_set(scope, name, (RValue)(uintptr_t)m);
    }
    return m;
}

RClass* rb_get_class(const char* name) {
    RValue v = rb_const_get(cObject, name);
    if (rb_is_ptr(v) && ((RObj*)(uintptr_t)v)->type == OBJ_CLASS) {
        return (RClass*)(uintptr_t)v;
    }
    return NULL;
}

void rb_def_method(RClass* klass, const char* name, RbFn fn, int arity) {
    if (!klass) klass = cObject;
    for (size_t i = 0; i < klass->num_methods; i++) {
        if (strcmp(klass->methods[i].name, name) == 0) {
            klass->methods[i].fn = fn;
            klass->methods[i].arity = arity;
            return;
        }
    }
    if (klass->num_methods >= klass->cap_methods) {
        klass->cap_methods = klass->cap_methods ? klass->cap_methods * 2 : 16;
        klass->methods = (RMethodEntry*)realloc(klass->methods, sizeof(RMethodEntry) * klass->cap_methods);
    }
    klass->methods[klass->num_methods].name = strdup(name);
    klass->methods[klass->num_methods].fn = fn;
    klass->methods[klass->num_methods].arity = arity;
    klass->num_methods++;
}

void rb_def_singleton_method(RValue obj, const char* name, RbFn fn, int arity) {
    for (size_t i = 0; i < g_num_singletons; i++) {
        if (g_singletons[i].obj == obj && strcmp(g_singletons[i].name, name) == 0) {
            g_singletons[i].fn = fn;
            g_singletons[i].arity = arity;
            return;
        }
    }
    if (g_num_singletons >= g_cap_singletons) {
        g_cap_singletons = g_cap_singletons ? g_cap_singletons * 2 : 32;
        g_singletons = (SingletonEntry*)realloc(g_singletons, sizeof(SingletonEntry) * g_cap_singletons);
    }
    g_singletons[g_num_singletons].obj = obj;
    g_singletons[g_num_singletons].name = strdup(name);
    g_singletons[g_num_singletons].fn = fn;
    g_singletons[g_num_singletons].arity = arity;
    g_num_singletons++;
}

void rb_include_module(RClass* klass, RClass* mod) {
    if (!klass || !mod) return;
    for (size_t i = 0; i < klass->num_includes; i++) {
        if (klass->includes[i] == mod) return;
    }
    if (klass->num_includes >= klass->cap_includes) {
        klass->cap_includes = klass->cap_includes ? klass->cap_includes * 2 : 4;
        klass->includes = (RClass**)realloc(klass->includes, sizeof(RClass*) * klass->cap_includes);
    }
    klass->includes[klass->num_includes++] = mod;
}

static RValue kv_get(RKVPair* arr, size_t len, const char* key) {
    for (size_t i = 0; i < len; i++) {
        if (strcmp(arr[i].key, key) == 0) return arr[i].val;
    }
    return QNIL;
}

static void kv_set(RKVPair** arr, size_t* len, size_t* cap, const char* key, RValue val) {
    for (size_t i = 0; i < *len; i++) {
        if (strcmp((*arr)[i].key, key) == 0) {
            (*arr)[i].val = val;
            return;
        }
    }
    if (*len >= *cap) {
        *cap = *cap ? *cap * 2 : 8;
        *arr = (RKVPair*)realloc(*arr, sizeof(RKVPair) * (*cap));
    }
    (*arr)[*len].key = strdup(key);
    (*arr)[*len].val = val;
    (*len)++;
}

RValue rb_ivar_get(RValue obj, const char* name) {
    if (!rb_is_ptr(obj)) return QNIL;
    RObj* o = (RObj*)(uintptr_t)obj;
    if (o->type == OBJ_INSTANCE) {
        RInstance* inst = (RInstance*)o;
        return kv_get(inst->ivars, inst->num_ivars, name);
    } else if (o->type == OBJ_CLASS) {
        RClass* c = (RClass*)o;
        return kv_get(c->ivars, c->num_ivars, name);
    }
    return QNIL;
}

RValue rb_ivar_set(RValue obj, const char* name, RValue val) {
    if (!rb_is_ptr(obj)) return val;
    RObj* o = (RObj*)(uintptr_t)obj;
    if (o->type == OBJ_INSTANCE) {
        RInstance* inst = (RInstance*)o;
        kv_set(&inst->ivars, &inst->num_ivars, &inst->cap_ivars, name, val);
    } else if (o->type == OBJ_CLASS) {
        RClass* c = (RClass*)o;
        kv_set(&c->ivars, &c->num_ivars, &c->cap_ivars, name, val);
    }
    return val;
}

RValue rb_cvar_get(RClass* klass, const char* name) {
    RClass* cur = klass;
    while (cur) {
        for (size_t i = 0; i < cur->num_cvars; i++) {
            if (strcmp(cur->cvars[i].key, name) == 0) return cur->cvars[i].val;
        }
        cur = cur->superclass;
    }
    return QNIL;
}

RValue rb_cvar_set(RClass* klass, const char* name, RValue val) {
    if (!klass) klass = cObject;
    kv_set(&klass->cvars, &klass->num_cvars, &klass->cap_cvars, name, val);
    return val;
}

RValue rb_gvar_get(const char* name) {
    return kv_get(g_gvars, g_num_gvars, name);
}

RValue rb_gvar_set(const char* name, RValue val) {
    kv_set(&g_gvars, &g_num_gvars, &g_cap_gvars, name, val);
    return val;
}

RValue rb_const_get(RClass* scope, const char* name) {
    RClass* cur = scope ? scope : cObject;
    while (cur) {
        for (size_t i = 0; i < cur->num_consts; i++) {
            if (strcmp(cur->consts[i].key, name) == 0) return cur->consts[i].val;
        }
        cur = cur->superclass;
    }
    if (scope && scope != cObject) {
        for (size_t i = 0; i < cObject->num_consts; i++) {
            if (strcmp(cObject->consts[i].key, name) == 0) return cObject->consts[i].val;
        }
    }
    char buf[256];
    snprintf(buf, sizeof(buf), "uninitialized constant %s", name);
    rb_raise_msg(cNameError, buf);
    return QNIL;
}

RValue rb_const_set(RClass* scope, const char* name, RValue val) {
    if (!scope) scope = cObject;
    kv_set(&scope->consts, &scope->num_consts, &scope->cap_consts, name, val);
    return val;
}

RValue rb_const_get_path(RValue parent, const char* name) {
    if (rb_is_ptr(parent) && ((RObj*)(uintptr_t)parent)->type == OBJ_CLASS) {
        RClass* c = (RClass*)(uintptr_t)parent;
        for (size_t i = 0; i < c->num_consts; i++) {
            if (strcmp(c->consts[i].key, name) == 0) return c->consts[i].val;
        }
    }
    char buf[256];
    snprintf(buf, sizeof(buf), "uninitialized constant %s", name);
    rb_raise_msg(cNameError, buf);
    return QNIL;
}

/* =========================================================================
 * Method Lookup & Dispatch
 * ========================================================================= */

static RMethodEntry* lookup_method_in_class(RClass* klass, const char* name) {
    while (klass) {
        for (size_t i = 0; i < klass->num_methods; i++) {
            if (strcmp(klass->methods[i].name, name) == 0) return &klass->methods[i];
        }
        for (size_t i = klass->num_includes; i > 0; i--) {
            RMethodEntry* m = lookup_method_in_class(klass->includes[i - 1], name);
            if (m) return m;
        }
        klass = klass->superclass;
    }
    return NULL;
}

RValue rb_send(RValue recv, const char* name, int64_t argc, RValue* argv, RValue block) {
    for (size_t i = 0; i < g_num_singletons; i++) {
        if (g_singletons[i].obj == recv && strcmp(g_singletons[i].name, name) == 0) {
            return g_singletons[i].fn(0, recv, argc, argv, block);
        }
    }
    RClass* klass = rb_class_of(recv);
    RMethodEntry* m = lookup_method_in_class(klass, name);
    if (!m) {
        char buf[256];
        snprintf(buf, sizeof(buf), "undefined method `%s' for %s", name, klass ? klass->name : "Object");
        rb_raise_msg(cNoMethodError, buf);
        return QNIL;
    }
    return m->fn(0, recv, argc, argv, block);
}

RValue rb_send_super(RClass* defining_klass, RValue recv, const char* name, int64_t argc, RValue* argv, RValue block) {
    RClass* start = defining_klass ? defining_klass->superclass : rb_class_of(recv)->superclass;
    RMethodEntry* m = lookup_method_in_class(start, name);
    if (!m) {
        char buf[256];
        snprintf(buf, sizeof(buf), "super: no superclass method `%s'", name);
        rb_raise_msg(cNoMethodError, buf);
        return QNIL;
    }
    return m->fn(0, recv, argc, argv, block);
}

RValue rb_proc_call(RValue proc_val, int64_t argc, RValue* argv, RValue block) {
    if (!rb_is_ptr(proc_val) || ((RObj*)(uintptr_t)proc_val)->type != OBJ_PROC) {
        rb_raise_msg(cRuntimeError, "no block given (yield)");
        return QNIL;
    }
    RProc* p = (RProc*)(uintptr_t)proc_val;
    return p->fn(p->env, p->captured_self, argc, argv, block);
}

void rb_check_args(int64_t argc, int min_args, int max_args, const char* name) {
    if (argc < min_args || (max_args >= 0 && argc > max_args)) {
        char buf[256];
        if (min_args == max_args) {
            snprintf(buf, sizeof(buf), "%s: wrong number of arguments (given %lld, expected %d)", name, (long long)argc, min_args);
        } else {
            snprintf(buf, sizeof(buf), "%s: wrong number of arguments (given %lld)", name, (long long)argc);
        }
        rb_raise_msg(cArgumentError, buf);
    }
}

/* =========================================================================
 * Exceptions
 * ========================================================================= */

void rb_push_rescue(jmp_buf* buf) {
    if (g_rescue_top >= MAX_RESCUE_DEPTH) {
        fprintf(stderr, "rubycom: rescue stack overflow\n");
        exit(1);
    }
    g_rescue_stack[g_rescue_top++] = buf;
}

void rb_pop_rescue(void) {
    if (g_rescue_top > 0) g_rescue_top--;
}

void rb_raise(RValue exc) {
    g_current_exc = exc;
    if (g_rescue_top > 0) {
        jmp_buf* buf = g_rescue_stack[--g_rescue_top];
        longjmp(*buf, 1);
    }
    RValue msg = rb_to_s(exc);
    RClass* k = rb_class_of(exc);
    fprintf(stderr, "Unhandled exception (%s): %s\n", k ? k->name : "Exception", ((RString*)(uintptr_t)msg)->data);
    exit(1);
}

void rb_raise_msg(RClass* klass, const char* msg) {
    RException* e = (RException*)rb_alloc_obj(OBJ_EXCEPTION, sizeof(RException));
    e->klass = klass ? klass : cRuntimeError;
    e->message = rb_str_new_cstr(msg);
    rb_raise((RValue)(uintptr_t)e);
}

RValue rb_current_exception(void) {
    return g_current_exc;
}

static int is_subclass_of(RClass* sub, RClass* sup) {
    while (sub) {
        if (sub == sup) return 1;
        for (size_t i = 0; i < sub->num_includes; i++) {
            if (is_subclass_of(sub->includes[i], sup)) return 1;
        }
        sub = sub->superclass;
    }
    return 0;
}

int rb_exception_matches(RValue exc, RValue klass_val) {
    if (!rb_is_ptr(klass_val) || ((RObj*)(uintptr_t)klass_val)->type != OBJ_CLASS) return 0;
    return is_subclass_of(rb_class_of(exc), (RClass*)(uintptr_t)klass_val);
}

/* =========================================================================
 * Core Conversions & Operators
 * ========================================================================= */

int rb_test(RValue v) {
    return rb_is_truthy(v);
}

RValue rb_to_s(RValue v) {
    if (v == QNIL) return rb_str_new_cstr("");
    if (v == QTRUE) return rb_str_new_cstr("true");
    if (v == QFALSE) return rb_str_new_cstr("false");
    if (rb_is_fixnum(v)) {
        char buf[64];
        snprintf(buf, sizeof(buf), "%lld", (long long)rb_fixnum_val(v));
        return rb_str_new_cstr(buf);
    }
    if (rb_is_symbol(v)) {
        return rb_str_new_cstr(rb_sym_name(v));
    }
    if (rb_is_ptr(v)) {
        RObj* o = (RObj*)(uintptr_t)v;
        if (o->type == OBJ_STRING) return v;
        if (o->type == OBJ_FLOAT) {
            char buf[64];
            double d = ((RFloat*)o)->val;
            if (isnan(d)) return rb_str_new_cstr("NaN");
            if (isinf(d)) return rb_str_new_cstr(d > 0 ? "Infinity" : "-Infinity");
            snprintf(buf, sizeof(buf), "%.15g", d);
            if (!strchr(buf, '.') && !strchr(buf, 'e') && !strchr(buf, 'E')) {
                strcat(buf, ".0");
            }
            return rb_str_new_cstr(buf);
        }
        return rb_send(v, "to_s", 0, NULL, QNIL);
    }
    return rb_str_new_cstr("");
}

RValue rb_inspect(RValue v) {
    if (v == QNIL) return rb_str_new_cstr("nil");
    if (v == QTRUE) return rb_str_new_cstr("true");
    if (v == QFALSE) return rb_str_new_cstr("false");
    if (rb_is_fixnum(v)) return rb_to_s(v);
    if (rb_is_symbol(v)) {
        const char* name = rb_sym_name(v);
        size_t len = strlen(name);
        char* buf = (char*)malloc(len + 2);
        buf[0] = ':';
        memcpy(buf + 1, name, len + 1);
        RValue res = rb_str_new_cstr(buf);
        free(buf);
        return res;
    }
    if (rb_is_ptr(v)) {
        return rb_send(v, "inspect", 0, NULL, QNIL);
    }
    return rb_str_new_cstr("nil");
}

static int is_float(RValue v) {
    return rb_is_ptr(v) && ((RObj*)(uintptr_t)v)->type == OBJ_FLOAT;
}

static double to_double(RValue v) {
    if (rb_is_fixnum(v)) return (double)rb_fixnum_val(v);
    if (is_float(v)) return ((RFloat*)(uintptr_t)v)->val;
    rb_raise_msg(cTypeError, "no implicit conversion to Float");
    return 0.0;
}

RValue rb_add(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) + rb_fixnum_val(b));
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return rb_float_new(to_double(a) + to_double(b));
    }
    return rb_send(a, "+", 1, &b, QNIL);
}

RValue rb_sub(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) - rb_fixnum_val(b));
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return rb_float_new(to_double(a) - to_double(b));
    }
    return rb_send(a, "-", 1, &b, QNIL);
}

RValue rb_mul(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) * rb_fixnum_val(b));
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return rb_float_new(to_double(a) * to_double(b));
    }
    return rb_send(a, "*", 1, &b, QNIL);
}

/* Ruby floor division for integers: -7 / 2 == -4, -7 % 2 == 1 */
static int64_t rb_int_div(int64_t a, int64_t b) {
    if (b == 0) rb_raise_msg(cZeroDivisionError, "divided by 0");
    int64_t q = a / b;
    int64_t r = a % b;
    if (r != 0 && ((r < 0) != (b < 0))) q--;
    return q;
}

static int64_t rb_int_mod(int64_t a, int64_t b) {
    if (b == 0) rb_raise_msg(cZeroDivisionError, "divided by 0");
    int64_t r = a % b;
    if (r != 0 && ((r < 0) != (b < 0))) r += b;
    return r;
}

RValue rb_div(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_int_div(rb_fixnum_val(a), rb_fixnum_val(b)));
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return rb_float_new(to_double(a) / to_double(b));
    }
    return rb_send(a, "/", 1, &b, QNIL);
}

RValue rb_mod(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_int_mod(rb_fixnum_val(a), rb_fixnum_val(b)));
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        double da = to_double(a), db = to_double(b);
        double r = fmod(da, db);
        if (r != 0.0 && ((r < 0.0) != (db < 0.0))) r += db;
        return rb_float_new(r);
    }
    return rb_send(a, "%", 1, &b, QNIL);
}

RValue rb_pow(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) {
        int64_t base = rb_fixnum_val(a);
        int64_t exp = rb_fixnum_val(b);
        if (exp >= 0) {
            int64_t res = 1;
            while (exp > 0) {
                if (exp & 1) res *= base;
                base *= base;
                exp >>= 1;
            }
            return rb_fixnum(res);
        }
        return rb_float_new(pow((double)base, (double)exp));
    }
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return rb_float_new(pow(to_double(a), to_double(b)));
    }
    return rb_send(a, "**", 1, &b, QNIL);
}

RValue rb_eq(RValue a, RValue b) {
    if (a == b) return QTRUE;
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return QFALSE;
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return to_double(a) == to_double(b) ? QTRUE : QFALSE;
    }
    if (!rb_is_ptr(a)) return QFALSE;
    return rb_send(a, "==", 1, &b, QNIL);
}

RValue rb_neq(RValue a, RValue b) {
    return rb_test(rb_eq(a, b)) ? QFALSE : QTRUE;
}

RValue rb_lt(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum_val(a) < rb_fixnum_val(b) ? QTRUE : QFALSE;
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return to_double(a) < to_double(b) ? QTRUE : QFALSE;
    }
    return rb_send(a, "<", 1, &b, QNIL);
}

RValue rb_le(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum_val(a) <= rb_fixnum_val(b) ? QTRUE : QFALSE;
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return to_double(a) <= to_double(b) ? QTRUE : QFALSE;
    }
    return rb_send(a, "<=", 1, &b, QNIL);
}

RValue rb_gt(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum_val(a) > rb_fixnum_val(b) ? QTRUE : QFALSE;
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return to_double(a) > to_double(b) ? QTRUE : QFALSE;
    }
    return rb_send(a, ">", 1, &b, QNIL);
}

RValue rb_ge(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum_val(a) >= rb_fixnum_val(b) ? QTRUE : QFALSE;
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        return to_double(a) >= to_double(b) ? QTRUE : QFALSE;
    }
    return rb_send(a, ">=", 1, &b, QNIL);
}

RValue rb_cmp(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) {
        int64_t va = rb_fixnum_val(a), vb = rb_fixnum_val(b);
        return rb_fixnum(va < vb ? -1 : (va > vb ? 1 : 0));
    }
    if ((rb_is_fixnum(a) || is_float(a)) && (rb_is_fixnum(b) || is_float(b))) {
        double da = to_double(a), db = to_double(b);
        return rb_fixnum(da < db ? -1 : (da > db ? 1 : 0));
    }
    return rb_send(a, "<=>", 1, &b, QNIL);
}

RValue rb_case_eq(RValue pattern, RValue target) {
    if (rb_is_ptr(pattern)) {
        return rb_send(pattern, "===", 1, &target, QNIL);
    }
    return rb_eq(pattern, target);
}

RValue rb_neg(RValue a) {
    if (rb_is_fixnum(a)) return rb_fixnum(-rb_fixnum_val(a));
    if (is_float(a)) return rb_float_new(-((RFloat*)(uintptr_t)a)->val);
    return rb_send(a, "-@", 0, NULL, QNIL);
}

RValue rb_pos(RValue a) {
    if (rb_is_fixnum(a) || is_float(a)) return a;
    return rb_send(a, "+@", 0, NULL, QNIL);
}

RValue rb_not(RValue a) {
    return rb_is_truthy(a) ? QFALSE : QTRUE;
}

RValue rb_bitnot(RValue a) {
    if (rb_is_fixnum(a)) return rb_fixnum(~rb_fixnum_val(a));
    return rb_send(a, "~", 0, NULL, QNIL);
}

RValue rb_bitand(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) & rb_fixnum_val(b));
    return rb_send(a, "&", 1, &b, QNIL);
}

RValue rb_bitor(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) | rb_fixnum_val(b));
    return rb_send(a, "|", 1, &b, QNIL);
}

RValue rb_bitxor(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) return rb_fixnum(rb_fixnum_val(a) ^ rb_fixnum_val(b));
    return rb_send(a, "^", 1, &b, QNIL);
}

RValue rb_shl(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) {
        int64_t shift = rb_fixnum_val(b);
        return shift >= 0 ? rb_fixnum(rb_fixnum_val(a) << shift) : rb_fixnum(rb_fixnum_val(a) >> (-shift));
    }
    return rb_send(a, "<<", 1, &b, QNIL);
}

RValue rb_shr(RValue a, RValue b) {
    if (rb_is_fixnum(a) && rb_is_fixnum(b)) {
        int64_t shift = rb_fixnum_val(b);
        return shift >= 0 ? rb_fixnum(rb_fixnum_val(a) >> shift) : rb_fixnum(rb_fixnum_val(a) << (-shift));
    }
    return rb_send(a, ">>", 1, &b, QNIL);
}

/* =========================================================================
 * Built-in Methods: Kernel / Object
 * ========================================================================= */

static void print_val_puts(RValue v) {
    if (v == QNIL) {
        putchar('\n');
        return;
    }
    if (rb_is_ptr(v) && ((RObj*)(uintptr_t)v)->type == OBJ_ARRAY) {
        RArray* a = (RArray*)(uintptr_t)v;
        for (size_t i = 0; i < a->len; i++) {
            print_val_puts(a->items[i]);
        }
        return;
    }
    RValue s = rb_to_s(v);
    RString* str = (RString*)(uintptr_t)s;
    fwrite(str->data, 1, str->len, stdout);
    if (str->len == 0 || str->data[str->len - 1] != '\n') {
        putchar('\n');
    }
}

static RValue m_puts(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    if (argc == 0) {
        putchar('\n');
        return QNIL;
    }
    for (int64_t i = 0; i < argc; i++) {
        print_val_puts(argv[i]);
    }
    return QNIL;
}

static RValue m_print(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    for (int64_t i = 0; i < argc; i++) {
        RValue s = rb_to_s(argv[i]);
        RString* str = (RString*)(uintptr_t)s;
        fwrite(str->data, 1, str->len, stdout);
    }
    return QNIL;
}

static RValue m_p(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    for (int64_t i = 0; i < argc; i++) {
        RValue s = rb_inspect(argv[i]);
        RString* str = (RString*)(uintptr_t)s;
        fwrite(str->data, 1, str->len, stdout);
        putchar('\n');
    }
    if (argc == 0) return QNIL;
    if (argc == 1) return argv[0];
    return rb_ary_new(argc, argv);
}

static RValue m_gets(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    char buf[4096];
    if (!fgets(buf, sizeof(buf), stdin)) return QNIL;
    return rb_str_new_cstr(buf);
}

static RValue m_obj_class(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return (RValue)(uintptr_t)rb_class_of(self);
}

static RValue m_obj_is_a(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "is_a?");
    if (!rb_is_ptr(argv[0]) || ((RObj*)(uintptr_t)argv[0])->type != OBJ_CLASS) {
        rb_raise_msg(cTypeError, "class or module required");
    }
    return is_subclass_of(rb_class_of(self), (RClass*)(uintptr_t)argv[0]) ? QTRUE : QFALSE;
}

static RValue m_obj_respond_to(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "respond_to?");
    RValue s = rb_to_s(argv[0]);
    const char* name = ((RString*)(uintptr_t)s)->data;
    for (size_t i = 0; i < g_num_singletons; i++) {
        if (g_singletons[i].obj == self && strcmp(g_singletons[i].name, name) == 0) return QTRUE;
    }
    return lookup_method_in_class(rb_class_of(self), name) ? QTRUE : QFALSE;
}

static RValue m_obj_send(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, -1, "send");
    RValue s = rb_to_s(argv[0]);
    const char* name = ((RString*)(uintptr_t)s)->data;
    return rb_send(self, name, argc - 1, argv + 1, block);
}

static RValue m_obj_nil_p(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return self == QNIL ? QTRUE : QFALSE;
}

static RValue m_obj_to_s(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RClass* k = rb_class_of(self);
    char buf[128];
    snprintf(buf, sizeof(buf), "#<%s:0x%llx>", k ? k->name : "Object", (unsigned long long)self);
    return rb_str_new_cstr(buf);
}

static RValue m_obj_inspect(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_send(self, "to_s", 0, NULL, QNIL);
}

static RValue m_obj_eq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "==");
    return self == argv[0] ? QTRUE : QFALSE;
}

static RValue m_obj_neq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "!=");
    return rb_test(rb_eq(self, argv[0])) ? QFALSE : QTRUE;
}

static RValue m_obj_id(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)(self >> 1));
}

static RValue m_raise(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    if (argc == 0) {
        if (g_current_exc != QNIL) rb_raise(g_current_exc);
        rb_raise_msg(cRuntimeError, "");
    } else if (argc == 1) {
        if (rb_is_ptr(argv[0]) && ((RObj*)(uintptr_t)argv[0])->type == OBJ_EXCEPTION) {
            rb_raise(argv[0]);
        } else if (rb_is_ptr(argv[0]) && ((RObj*)(uintptr_t)argv[0])->type == OBJ_CLASS) {
            rb_raise_msg((RClass*)(uintptr_t)argv[0], ((RClass*)(uintptr_t)argv[0])->name);
        } else {
            RValue s = rb_to_s(argv[0]);
            rb_raise_msg(cRuntimeError, ((RString*)(uintptr_t)s)->data);
        }
    } else {
        RClass* k = (rb_is_ptr(argv[0]) && ((RObj*)(uintptr_t)argv[0])->type == OBJ_CLASS)
            ? (RClass*)(uintptr_t)argv[0] : cRuntimeError;
        RValue s = rb_to_s(argv[1]);
        rb_raise_msg(k, ((RString*)(uintptr_t)s)->data);
    }
    return QNIL;
}

static RValue m_exit(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    int code = 0;
    if (argc >= 1) {
        if (rb_is_fixnum(argv[0])) code = (int)rb_fixnum_val(argv[0]);
        else if (argv[0] == QFALSE) code = 1;
    }
    exit(code);
    return QNIL;
}

static RValue m_rand(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    if (argc == 0) {
        return rb_float_new((double)rand() / ((double)RAND_MAX + 1.0));
    }
    int64_t max = rb_fixnum_val(argv[0]);
    if (max <= 0) return rb_fixnum(0);
    return rb_fixnum(rand() % max);
}

static RValue m_srand(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    unsigned int seed = argc >= 1 ? (unsigned int)rb_fixnum_val(argv[0]) : (unsigned int)time(NULL);
    srand(seed);
    return rb_fixnum(seed);
}

static RValue m_conv_int(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Integer");
    return rb_send(argv[0], "to_i", 0, NULL, QNIL);
}

static RValue m_conv_float(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Float");
    return rb_send(argv[0], "to_f", 0, NULL, QNIL);
}

static RValue m_conv_str(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String");
    return rb_to_s(argv[0]);
}

static RValue m_conv_ary(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Array");
    if (rb_is_ptr(argv[0]) && ((RObj*)(uintptr_t)argv[0])->type == OBJ_ARRAY) return argv[0];
    if (rb_is_ptr(argv[0]) && ((RObj*)(uintptr_t)argv[0])->type == OBJ_RANGE) {
        return rb_send(argv[0], "to_a", 0, NULL, QNIL);
    }
    return rb_ary_new(1, &argv[0]);
}

/* =========================================================================
 * Built-in Methods: Integer & Float
 * ========================================================================= */

static RValue m_int_times(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    int64_t n = rb_fixnum_val(self);
    for (int64_t i = 0; i < n; i++) {
        RValue arg = rb_fixnum(i);
        rb_proc_call(block, 1, &arg, QNIL);
    }
    return self;
}

static RValue m_int_upto(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "upto");
    int64_t a = rb_fixnum_val(self), b = rb_fixnum_val(argv[0]);
    for (int64_t i = a; i <= b; i++) {
        RValue arg = rb_fixnum(i);
        rb_proc_call(block, 1, &arg, QNIL);
    }
    return self;
}

static RValue m_int_downto(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "downto");
    int64_t a = rb_fixnum_val(self), b = rb_fixnum_val(argv[0]);
    for (int64_t i = a; i >= b; i--) {
        RValue arg = rb_fixnum(i);
        rb_proc_call(block, 1, &arg, QNIL);
    }
    return self;
}

static RValue m_int_abs(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    int64_t v = rb_fixnum_val(self);
    return rb_fixnum(v < 0 ? -v : v);
}

static RValue m_int_even(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return (rb_fixnum_val(self) % 2 == 0) ? QTRUE : QFALSE;
}

static RValue m_int_odd(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return (rb_fixnum_val(self) % 2 != 0) ? QTRUE : QFALSE;
}

static RValue m_int_zero(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum_val(self) == 0 ? QTRUE : QFALSE;
}

static RValue m_int_to_i(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return self; }
static RValue m_int_to_f(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return rb_float_new((double)rb_fixnum_val(self)); }
static RValue m_int_to_s(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return rb_to_s(self); }
static RValue m_int_succ(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return rb_fixnum(rb_fixnum_val(self) + 1); }
static RValue m_int_pred(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return rb_fixnum(rb_fixnum_val(self) - 1); }
static RValue m_int_chr(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    char c = (char)rb_fixnum_val(self);
    return rb_str_new(&c, 1);
}

static RValue m_flt_abs(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_float_new(fabs(((RFloat*)(uintptr_t)self)->val));
}
static RValue m_flt_round(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    double d = ((RFloat*)(uintptr_t)self)->val;
    if (argc == 0) return rb_fixnum((int64_t)llround(d));
    int64_t digits = rb_fixnum_val(argv[0]);
    double p = pow(10.0, (double)digits);
    return rb_float_new(round(d * p) / p);
}
static RValue m_flt_floor(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)floor(((RFloat*)(uintptr_t)self)->val));
}
static RValue m_flt_ceil(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)ceil(((RFloat*)(uintptr_t)self)->val));
}
static RValue m_flt_to_i(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)((RFloat*)(uintptr_t)self)->val);
}
static RValue m_flt_to_f(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return self; }
static RValue m_flt_to_s(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return rb_to_s(self); }

/* =========================================================================
 * Built-in Methods: Math
 * ========================================================================= */

static RValue m_math_sqrt(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.sqrt");
    return rb_float_new(sqrt(to_double(argv[0])));
}
static RValue m_math_sin(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.sin");
    return rb_float_new(sin(to_double(argv[0])));
}
static RValue m_math_cos(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.cos");
    return rb_float_new(cos(to_double(argv[0])));
}
static RValue m_math_tan(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.tan");
    return rb_float_new(tan(to_double(argv[0])));
}
static RValue m_math_log(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.log");
    return rb_float_new(log(to_double(argv[0])));
}
static RValue m_math_exp(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Math.exp");
    return rb_float_new(exp(to_double(argv[0])));
}

/* =========================================================================
 * Built-in Methods: String
 * ========================================================================= */

static RValue m_str_add(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#+");
    return rb_str_concat(self, argv[0]);
}

static RValue m_str_mul(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#*");
    RString* s = (RString*)(uintptr_t)self;
    int64_t count = rb_fixnum_val(argv[0]);
    if (count <= 0) return rb_str_new_cstr("");
    size_t nlen = s->len * (size_t)count;
    char* buf = (char*)malloc(nlen + 1);
    for (int64_t i = 0; i < count; i++) {
        memcpy(buf + (size_t)i * s->len, s->data, s->len);
    }
    buf[nlen] = '\0';
    RValue res = rb_str_new(buf, nlen);
    free(buf);
    return res;
}

static RValue m_str_shl(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#<<");
    RString* s = (RString*)(uintptr_t)self;
    RValue other = rb_to_s(argv[0]);
    RString* o = (RString*)(uintptr_t)other;
    size_t nlen = s->len + o->len;
    if (nlen + 1 > s->cap) {
        s->cap = nlen + 16;
        s->data = (char*)realloc(s->data, s->cap);
    }
    memcpy(s->data + s->len, o->data, o->len);
    s->len = nlen;
    s->data[nlen] = '\0';
    return self;
}

static RValue m_str_eq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#==");
    if (!rb_is_ptr(argv[0]) || ((RObj*)(uintptr_t)argv[0])->type != OBJ_STRING) return QFALSE;
    RString* a = (RString*)(uintptr_t)self;
    RString* b = (RString*)(uintptr_t)argv[0];
    if (a->len != b->len) return QFALSE;
    return memcmp(a->data, b->data, a->len) == 0 ? QTRUE : QFALSE;
}

static int str_cmp_raw(RString* a, RString* b) {
    size_t minl = a->len < b->len ? a->len : b->len;
    int r = memcmp(a->data, b->data, minl);
    if (r != 0) return r < 0 ? -1 : 1;
    if (a->len < b->len) return -1;
    if (a->len > b->len) return 1;
    return 0;
}

static RValue m_str_cmp(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#<=>");
    if (!rb_is_ptr(argv[0]) || ((RObj*)(uintptr_t)argv[0])->type != OBJ_STRING) return QNIL;
    return rb_fixnum(str_cmp_raw((RString*)(uintptr_t)self, (RString*)(uintptr_t)argv[0]));
}

static RValue m_str_lt(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return str_cmp_raw((RString*)(uintptr_t)self, (RString*)(uintptr_t)argv[0]) < 0 ? QTRUE : QFALSE;
}
static RValue m_str_le(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return str_cmp_raw((RString*)(uintptr_t)self, (RString*)(uintptr_t)argv[0]) <= 0 ? QTRUE : QFALSE;
}
static RValue m_str_gt(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return str_cmp_raw((RString*)(uintptr_t)self, (RString*)(uintptr_t)argv[0]) > 0 ? QTRUE : QFALSE;
}
static RValue m_str_ge(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return str_cmp_raw((RString*)(uintptr_t)self, (RString*)(uintptr_t)argv[0]) >= 0 ? QTRUE : QFALSE;
}

static RValue m_str_len(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)((RString*)(uintptr_t)self)->len);
}

static RValue m_str_empty(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return ((RString*)(uintptr_t)self)->len == 0 ? QTRUE : QFALSE;
}

static RValue m_str_aref(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    if (argc == 1 && rb_is_fixnum(argv[0])) {
        int64_t idx = rb_fixnum_val(argv[0]);
        if (idx < 0) idx += (int64_t)s->len;
        if (idx < 0 || (size_t)idx >= s->len) return QNIL;
        return rb_str_new(&s->data[idx], 1);
    }
    if (argc == 2) {
        int64_t idx = rb_fixnum_val(argv[0]);
        int64_t len = rb_fixnum_val(argv[1]);
        if (idx < 0) idx += (int64_t)s->len;
        if (idx < 0 || (size_t)idx > s->len || len < 0) return QNIL;
        if ((size_t)(idx + len) > s->len) len = (int64_t)s->len - idx;
        return rb_str_new(s->data + idx, (size_t)len);
    }
    return QNIL;
}

static RValue m_str_upcase(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue res = rb_str_new(s->data, s->len);
    RString* r = (RString*)(uintptr_t)res;
    for (size_t i = 0; i < r->len; i++) r->data[i] = (char)toupper((unsigned char)r->data[i]);
    return res;
}

static RValue m_str_downcase(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue res = rb_str_new(s->data, s->len);
    RString* r = (RString*)(uintptr_t)res;
    for (size_t i = 0; i < r->len; i++) r->data[i] = (char)tolower((unsigned char)r->data[i]);
    return res;
}

static RValue m_str_reverse(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue res = rb_str_new(s->data, s->len);
    RString* r = (RString*)(uintptr_t)res;
    for (size_t i = 0; i < r->len / 2; i++) {
        char tmp = r->data[i];
        r->data[i] = r->data[r->len - 1 - i];
        r->data[r->len - 1 - i] = tmp;
    }
    return res;
}

static RValue m_str_strip(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    size_t start = 0, end = s->len;
    while (start < end && isspace((unsigned char)s->data[start])) start++;
    while (end > start && isspace((unsigned char)s->data[end - 1])) end--;
    return rb_str_new(s->data + start, end - start);
}

static RValue m_str_chomp(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    size_t len = s->len;
    if (len > 0 && s->data[len - 1] == '\n') len--;
    if (len > 0 && s->data[len - 1] == '\r') len--;
    return rb_str_new(s->data, len);
}

static RValue m_str_split(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue ary = rb_ary_new(0, NULL);
    if (argc == 0 || argv[0] == QNIL) {
        size_t i = 0;
        while (i < s->len) {
            while (i < s->len && isspace((unsigned char)s->data[i])) i++;
            if (i >= s->len) break;
            size_t start = i;
            while (i < s->len && !isspace((unsigned char)s->data[i])) i++;
            rb_ary_push(ary, rb_str_new(s->data + start, i - start));
        }
        return ary;
    }
    RValue sep_val = rb_to_s(argv[0]);
    RString* sep = (RString*)(uintptr_t)sep_val;
    if (sep->len == 0) {
        for (size_t i = 0; i < s->len; i++) rb_ary_push(ary, rb_str_new(s->data + i, 1));
        return ary;
    }
    const char* p = s->data;
    const char* end = s->data + s->len;
    while (p <= end) {
        const char* found = strstr(p, sep->data);
        if (!found) {
            rb_ary_push(ary, rb_str_new(p, (size_t)(end - p)));
            break;
        }
        rb_ary_push(ary, rb_str_new(p, (size_t)(found - p)));
        p = found + sep->len;
    }
    return ary;
}

static RValue m_str_chars(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue ary = rb_ary_new(0, NULL);
    for (size_t i = 0; i < s->len; i++) rb_ary_push(ary, rb_str_new(s->data + i, 1));
    return ary;
}

static RValue m_str_include(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#include?");
    RString* s = (RString*)(uintptr_t)self;
    RString* sub = (RString*)(uintptr_t)rb_to_s(argv[0]);
    return strstr(s->data, sub->data) ? QTRUE : QFALSE;
}

static RValue m_str_start_with(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#start_with?");
    RString* s = (RString*)(uintptr_t)self;
    RString* sub = (RString*)(uintptr_t)rb_to_s(argv[0]);
    if (sub->len > s->len) return QFALSE;
    return memcmp(s->data, sub->data, sub->len) == 0 ? QTRUE : QFALSE;
}

static RValue m_str_end_with(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "String#end_with?");
    RString* s = (RString*)(uintptr_t)self;
    RString* sub = (RString*)(uintptr_t)rb_to_s(argv[0]);
    if (sub->len > s->len) return QFALSE;
    return memcmp(s->data + (s->len - sub->len), sub->data, sub->len) == 0 ? QTRUE : QFALSE;
}

static RValue m_str_to_i(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    int base = argc >= 1 ? (int)rb_fixnum_val(argv[0]) : 10;
    return rb_fixnum((int64_t)strtoll(((RString*)(uintptr_t)self)->data, NULL, base));
}

static RValue m_str_to_f(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_float_new(strtod(((RString*)(uintptr_t)self)->data, NULL));
}

static RValue m_str_to_s(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) { return self; }
static RValue m_str_to_sym(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_sym_intern(((RString*)(uintptr_t)self)->data);
}

static RValue m_str_inspect(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    RValue out = rb_str_new_cstr("\"");
    for (size_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        if (c == '"') m_str_shl(0, out, 1, (RValue[]){rb_str_new_cstr("\\\"")}, QNIL);
        else if (c == '\\') m_str_shl(0, out, 1, (RValue[]){rb_str_new_cstr("\\\\")}, QNIL);
        else if (c == '\n') m_str_shl(0, out, 1, (RValue[]){rb_str_new_cstr("\\n")}, QNIL);
        else if (c == '\t') m_str_shl(0, out, 1, (RValue[]){rb_str_new_cstr("\\t")}, QNIL);
        else m_str_shl(0, out, 1, (RValue[]){rb_str_new(&c, 1)}, QNIL);
    }
    m_str_shl(0, out, 1, (RValue[]){rb_str_new_cstr("\"")}, QNIL);
    return out;
}

static RValue m_str_ord(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RString* s = (RString*)(uintptr_t)self;
    if (s->len == 0) rb_raise_msg(cArgumentError, "empty string");
    return rb_fixnum((unsigned char)s->data[0]);
}

static RValue m_str_gsub(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 2, 2, "String#gsub");
    RString* s = (RString*)(uintptr_t)self;
    RString* pat = (RString*)(uintptr_t)rb_to_s(argv[0]);
    RString* rep = (RString*)(uintptr_t)rb_to_s(argv[1]);
    if (pat->len == 0) return self;
    RValue out = rb_str_new_cstr("");
    const char* p = s->data;
    const char* end = s->data + s->len;
    while (p < end) {
        const char* f = strstr(p, pat->data);
        if (!f) {
            RValue chunk = rb_str_new(p, (size_t)(end - p));
            m_str_shl(0, out, 1, &chunk, QNIL);
            break;
        }
        if (f > p) {
            RValue chunk = rb_str_new(p, (size_t)(f - p));
            m_str_shl(0, out, 1, &chunk, QNIL);
        }
        RValue rchunk = rb_str_new(rep->data, rep->len);
        m_str_shl(0, out, 1, &rchunk, QNIL);
        p = f + pat->len;
    }
    return out;
}

/* =========================================================================
 * Built-in Methods: Array
 * ========================================================================= */

static RValue m_ary_aref(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    if (argc == 1) {
        int64_t idx = rb_fixnum_val(argv[0]);
        if (idx < 0) idx += (int64_t)a->len;
        if (idx < 0 || (size_t)idx >= a->len) return QNIL;
        return a->items[idx];
    }
    if (argc == 2) {
        int64_t idx = rb_fixnum_val(argv[0]);
        int64_t len = rb_fixnum_val(argv[1]);
        if (idx < 0) idx += (int64_t)a->len;
        if (idx < 0 || (size_t)idx > a->len || len < 0) return QNIL;
        if ((size_t)(idx + len) > a->len) len = (int64_t)a->len - idx;
        return rb_ary_new(len, a->items + idx);
    }
    return QNIL;
}

static RValue m_ary_aset(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 2, 2, "Array#[]=");
    RArray* a = (RArray*)(uintptr_t)self;
    int64_t idx = rb_fixnum_val(argv[0]);
    RValue val = argv[1];
    if (idx < 0) idx += (int64_t)a->len;
    if (idx < 0) rb_raise_msg(cIndexError, "index too small for array");
    while ((size_t)idx >= a->len) rb_ary_push(self, QNIL);
    a->items[idx] = val;
    return val;
}

static RValue m_ary_len(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)((RArray*)(uintptr_t)self)->len);
}

static RValue m_ary_empty(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return ((RArray*)(uintptr_t)self)->len == 0 ? QTRUE : QFALSE;
}

static RValue m_ary_push(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    for (int64_t i = 0; i < argc; i++) rb_ary_push(self, argv[i]);
    return self;
}

static RValue m_ary_pop(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    if (a->len == 0) return QNIL;
    return a->items[--a->len];
}

static RValue m_ary_shift(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    if (a->len == 0) return QNIL;
    RValue first = a->items[0];
    memmove(a->items, a->items + 1, sizeof(RValue) * (--a->len));
    return first;
}

static RValue m_ary_unshift(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (int64_t i = argc - 1; i >= 0; i--) {
        rb_ary_push(self, QNIL);
        memmove(a->items + 1, a->items, sizeof(RValue) * (a->len - 1));
        a->items[0] = argv[i];
    }
    return self;
}

static RValue m_ary_first(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    return a->len > 0 ? a->items[0] : QNIL;
}

static RValue m_ary_last(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    return a->len > 0 ? a->items[a->len - 1] : QNIL;
}

static RValue m_ary_include(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Array#include?");
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        if (rb_test(rb_eq(a->items[i], argv[0]))) return QTRUE;
    }
    return QFALSE;
}

static RValue m_ary_index(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Array#index");
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        if (rb_test(rb_eq(a->items[i], argv[0]))) return rb_fixnum((int64_t)i);
    }
    return QNIL;
}

static RValue m_ary_each(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) rb_proc_call(block, 1, &a->items[i], QNIL);
    return self;
}

static RValue m_ary_each_with_index(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        RValue args[2] = { a->items[i], rb_fixnum((int64_t)i) };
        rb_proc_call(block, 2, args, QNIL);
    }
    return self;
}

static RValue m_ary_map(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < a->len; i++) {
        rb_ary_push(out, rb_proc_call(block, 1, &a->items[i], QNIL));
    }
    return out;
}

static RValue m_ary_select(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < a->len; i++) {
        if (rb_test(rb_proc_call(block, 1, &a->items[i], QNIL))) rb_ary_push(out, a->items[i]);
    }
    return out;
}

static RValue m_ary_reject(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < a->len; i++) {
        if (!rb_test(rb_proc_call(block, 1, &a->items[i], QNIL))) rb_ary_push(out, a->items[i]);
    }
    return out;
}

static RValue m_ary_reduce(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    size_t start = 0;
    RValue acc;
    if (argc >= 1) {
        acc = argv[0];
    } else {
        if (a->len == 0) return QNIL;
        acc = a->items[0];
        start = 1;
    }
    for (size_t i = start; i < a->len; i++) {
        RValue args[2] = { acc, a->items[i] };
        acc = rb_proc_call(block, 2, args, QNIL);
    }
    return acc;
}

static RValue m_ary_any(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        RValue v = block != QNIL ? rb_proc_call(block, 1, &a->items[i], QNIL) : a->items[i];
        if (rb_test(v)) return QTRUE;
    }
    return QFALSE;
}

static RValue m_ary_all(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        RValue v = block != QNIL ? rb_proc_call(block, 1, &a->items[i], QNIL) : a->items[i];
        if (!rb_test(v)) return QFALSE;
    }
    return QTRUE;
}

static RValue m_ary_find(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    for (size_t i = 0; i < a->len; i++) {
        if (rb_test(rb_proc_call(block, 1, &a->items[i], QNIL))) return a->items[i];
    }
    return QNIL;
}

static RValue m_ary_sum(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue acc = argc >= 1 ? argv[0] : rb_fixnum(0);
    for (size_t i = 0; i < a->len; i++) {
        RValue item = block != QNIL ? rb_proc_call(block, 1, &a->items[i], QNIL) : a->items[i];
        acc = rb_add(acc, item);
    }
    return acc;
}

static RValue m_ary_min(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    if (a->len == 0) return QNIL;
    RValue best = a->items[0];
    for (size_t i = 1; i < a->len; i++) {
        if (rb_test(rb_lt(a->items[i], best))) best = a->items[i];
    }
    return best;
}

static RValue m_ary_max(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    if (a->len == 0) return QNIL;
    RValue best = a->items[0];
    for (size_t i = 1; i < a->len; i++) {
        if (rb_test(rb_gt(a->items[i], best))) best = a->items[i];
    }
    return best;
}

static RValue m_ary_reverse(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = a->len; i > 0; i--) rb_ary_push(out, a->items[i - 1]);
    return out;
}

static int ary_qsort_cmp(const void* pa, const void* pb) {
    RValue a = *(const RValue*)pa;
    RValue b = *(const RValue*)pb;
    RValue c = rb_cmp(a, b);
    return (int)rb_fixnum_val(c);
}

static RValue m_ary_sort(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new((int64_t)a->len, a->items);
    RArray* o = (RArray*)(uintptr_t)out;
    if (o->len > 1) qsort(o->items, o->len, sizeof(RValue), ary_qsort_cmp);
    return out;
}

static RValue m_ary_uniq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < a->len; i++) {
        if (!rb_test(m_ary_include(0, out, 1, &a->items[i], QNIL))) rb_ary_push(out, a->items[i]);
    }
    return out;
}

static RValue m_ary_compact(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < a->len; i++) {
        if (a->items[i] != QNIL) rb_ary_push(out, a->items[i]);
    }
    return out;
}

static RValue m_ary_join(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue sep = argc >= 1 ? rb_to_s(argv[0]) : rb_str_new_cstr("");
    RValue out = rb_str_new_cstr("");
    for (size_t i = 0; i < a->len; i++) {
        if (i > 0) m_str_shl(0, out, 1, &sep, QNIL);
        RValue item = rb_to_s(a->items[i]);
        m_str_shl(0, out, 1, &item, QNIL);
    }
    return out;
}

static RValue m_ary_add(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Array#+");
    RArray* a = (RArray*)(uintptr_t)self;
    RArray* b = (RArray*)(uintptr_t)argv[0];
    RValue out = rb_ary_new((int64_t)a->len, a->items);
    for (size_t i = 0; i < b->len; i++) rb_ary_push(out, b->items[i]);
    return out;
}

static RValue m_ary_eq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    if (!rb_is_ptr(argv[0]) || ((RObj*)(uintptr_t)argv[0])->type != OBJ_ARRAY) return QFALSE;
    RArray* a = (RArray*)(uintptr_t)self;
    RArray* b = (RArray*)(uintptr_t)argv[0];
    if (a->len != b->len) return QFALSE;
    for (size_t i = 0; i < a->len; i++) {
        if (!rb_test(rb_eq(a->items[i], b->items[i]))) return QFALSE;
    }
    return QTRUE;
}

static RValue m_ary_inspect(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RArray* a = (RArray*)(uintptr_t)self;
    RValue out = rb_str_new_cstr("[");
    RValue comma = rb_str_new_cstr(", ");
    for (size_t i = 0; i < a->len; i++) {
        if (i > 0) m_str_shl(0, out, 1, &comma, QNIL);
        RValue item = rb_inspect(a->items[i]);
        m_str_shl(0, out, 1, &item, QNIL);
    }
    RValue close = rb_str_new_cstr("]");
    m_str_shl(0, out, 1, &close, QNIL);
    return out;
}

/* =========================================================================
 * Built-in Methods: Hash
 * ========================================================================= */

static RValue m_hash_aref(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Hash#[]");
    RHash* h = (RHash*)(uintptr_t)self;
    for (size_t i = 0; i < h->len; i++) {
        if (rb_test(rb_eq(h->entries[i].key, argv[0]))) return h->entries[i].val;
    }
    return QNIL;
}

static RValue m_hash_aset(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 2, 2, "Hash#[]=");
    RHash* h = (RHash*)(uintptr_t)self;
    for (size_t i = 0; i < h->len; i++) {
        if (rb_test(rb_eq(h->entries[i].key, argv[0]))) {
            h->entries[i].val = argv[1];
            return argv[1];
        }
    }
    if (h->len >= h->cap) {
        h->cap = h->cap ? h->cap * 2 : 8;
        h->entries = (RHashEntry*)realloc(h->entries, sizeof(RHashEntry) * h->cap);
    }
    h->entries[h->len].key = argv[0];
    h->entries[h->len].val = argv[1];
    h->len++;
    return argv[1];
}

static RValue m_hash_len(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_fixnum((int64_t)((RHash*)(uintptr_t)self)->len);
}

static RValue m_hash_empty(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return ((RHash*)(uintptr_t)self)->len == 0 ? QTRUE : QFALSE;
}

static RValue m_hash_keys(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RHash* h = (RHash*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < h->len; i++) rb_ary_push(out, h->entries[i].key);
    return out;
}

static RValue m_hash_values(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RHash* h = (RHash*)(uintptr_t)self;
    RValue out = rb_ary_new(0, NULL);
    for (size_t i = 0; i < h->len; i++) rb_ary_push(out, h->entries[i].val);
    return out;
}

static RValue m_hash_has_key(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Hash#has_key?");
    RHash* h = (RHash*)(uintptr_t)self;
    for (size_t i = 0; i < h->len; i++) {
        if (rb_test(rb_eq(h->entries[i].key, argv[0]))) return QTRUE;
    }
    return QFALSE;
}

static RValue m_hash_delete(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Hash#delete");
    RHash* h = (RHash*)(uintptr_t)self;
    for (size_t i = 0; i < h->len; i++) {
        if (rb_test(rb_eq(h->entries[i].key, argv[0]))) {
            RValue val = h->entries[i].val;
            memmove(&h->entries[i], &h->entries[i + 1], sizeof(RHashEntry) * (h->len - 1 - i));
            h->len--;
            return val;
        }
    }
    return QNIL;
}

static RValue m_hash_each(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RHash* h = (RHash*)(uintptr_t)self;
    for (size_t i = 0; i < h->len; i++) {
        RValue args[2] = { h->entries[i].key, h->entries[i].val };
        rb_proc_call(block, 2, args, QNIL);
    }
    return self;
}

static RValue m_hash_inspect(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RHash* h = (RHash*)(uintptr_t)self;
    RValue out = rb_str_new_cstr("{");
    RValue comma = rb_str_new_cstr(", ");
    RValue arrow = rb_str_new_cstr("=>");
    for (size_t i = 0; i < h->len; i++) {
        if (i > 0) m_str_shl(0, out, 1, &comma, QNIL);
        RValue k = rb_inspect(h->entries[i].key);
        RValue v = rb_inspect(h->entries[i].val);
        m_str_shl(0, out, 1, &k, QNIL);
        m_str_shl(0, out, 1, &arrow, QNIL);
        m_str_shl(0, out, 1, &v, QNIL);
    }
    RValue close = rb_str_new_cstr("}");
    m_str_shl(0, out, 1, &close, QNIL);
    return out;
}

/* =========================================================================
 * Built-in Methods: Range, Proc, Class, Exception, File
 * ========================================================================= */

static RValue m_range_each(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RRange* r = (RRange*)(uintptr_t)self;
    int64_t s = rb_fixnum_val(r->start);
    int64_t e = rb_fixnum_val(r->end);
    int64_t limit = r->exclusive ? e - 1 : e;
    for (int64_t i = s; i <= limit; i++) {
        RValue arg = rb_fixnum(i);
        rb_proc_call(block, 1, &arg, QNIL);
    }
    return self;
}

static RValue m_range_to_a(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RRange* r = (RRange*)(uintptr_t)self;
    int64_t s = rb_fixnum_val(r->start);
    int64_t e = rb_fixnum_val(r->end);
    int64_t limit = r->exclusive ? e - 1 : e;
    RValue out = rb_ary_new(0, NULL);
    for (int64_t i = s; i <= limit; i++) rb_ary_push(out, rb_fixnum(i));
    return out;
}

static RValue m_range_map(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RValue ary = m_range_to_a(0, self, 0, NULL, QNIL);
    return m_ary_map(0, ary, 0, NULL, block);
}

static RValue m_range_include(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Range#include?");
    RRange* r = (RRange*)(uintptr_t)self;
    if (!rb_test(rb_ge(argv[0], r->start))) return QFALSE;
    return r->exclusive ? rb_lt(argv[0], r->end) : rb_le(argv[0], r->end);
}

static RValue m_proc_call(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_proc_call(self, argc, argv, block);
}

static RValue m_class_new(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    RClass* klass = (RClass*)(uintptr_t)self;
    if (is_subclass_of(klass, cException)) {
        RException* e = (RException*)rb_alloc_obj(OBJ_EXCEPTION, sizeof(RException));
        e->klass = klass;
        e->message = argc >= 1 ? rb_to_s(argv[0]) : rb_str_new_cstr(klass->name);
        return (RValue)(uintptr_t)e;
    }
    RInstance* inst = (RInstance*)rb_alloc_obj(OBJ_INSTANCE, sizeof(RInstance));
    inst->klass = klass;
    RValue obj = (RValue)(uintptr_t)inst;
    if (lookup_method_in_class(klass, "initialize")) {
        rb_send(obj, "initialize", argc, argv, block);
    }
    return obj;
}

static RValue m_class_name(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_str_new_cstr(((RClass*)(uintptr_t)self)->name);
}

static RValue m_class_case_eq(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "Module#===");
    return is_subclass_of(rb_class_of(argv[0]), (RClass*)(uintptr_t)self) ? QTRUE : QFALSE;
}

static RValue m_exc_message(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return ((RException*)(uintptr_t)self)->message;
}

static RValue m_sym_to_s(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    return rb_str_new_cstr(rb_sym_name(self));
}

static RValue m_file_read(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "File.read");
    const char* path = ((RString*)(uintptr_t)rb_to_s(argv[0]))->data;
    FILE* f = fopen(path, "rb");
    if (!f) rb_raise_msg(cRuntimeError, "No such file or directory");
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    char* buf = (char*)malloc((size_t)sz + 1);
    size_t n = fread(buf, 1, (size_t)sz, f);
    buf[n] = '\0';
    fclose(f);
    RValue res = rb_str_new(buf, n);
    free(buf);
    return res;
}

static RValue m_file_write(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 2, 2, "File.write");
    const char* path = ((RString*)(uintptr_t)rb_to_s(argv[0]))->data;
    RString* content = (RString*)(uintptr_t)rb_to_s(argv[1]);
    FILE* f = fopen(path, "wb");
    if (!f) rb_raise_msg(cRuntimeError, "Cannot open file for writing");
    size_t n = fwrite(content->data, 1, content->len, f);
    fclose(f);
    return rb_fixnum((int64_t)n);
}

static RValue m_file_exist(RValue env, RValue self, int64_t argc, RValue* argv, RValue block) {
    rb_check_args(argc, 1, 1, "File.exist?");
    const char* path = ((RString*)(uintptr_t)rb_to_s(argv[0]))->data;
    return access(path, F_OK) == 0 ? QTRUE : QFALSE;
}

/* =========================================================================
 * Initialization
 * ========================================================================= */

static RClass* make_bootstrap_class(const char* name, RClass* super, int is_mod) {
    RClass* c = (RClass*)rb_alloc_obj(OBJ_CLASS, sizeof(RClass));
    c->name = strdup(name);
    c->superclass = super;
    c->is_module = is_mod;
    return c;
}

void rb_init(int argc, char** argv) {
    volatile int stack_anchor = 0;
    g_stack_bottom = (uintptr_t)&stack_anchor;

    cBasicObject = make_bootstrap_class("BasicObject", NULL, 0);
    cObject = make_bootstrap_class("Object", cBasicObject, 0);
    cModule = make_bootstrap_class("Module", cObject, 0);
    cClass = make_bootstrap_class("Class", cModule, 0);

    rb_const_set(cObject, "BasicObject", (RValue)(uintptr_t)cBasicObject);
    rb_const_set(cObject, "Object", (RValue)(uintptr_t)cObject);
    rb_const_set(cObject, "Module", (RValue)(uintptr_t)cModule);
    rb_const_set(cObject, "Class", (RValue)(uintptr_t)cClass);

    mKernel = rb_define_module("Kernel", cObject);
    rb_include_module(cObject, mKernel);

    cInteger = rb_define_class("Integer", cObject, cObject);
    cFloat = rb_define_class("Float", cObject, cObject);
    cString = rb_define_class("String", cObject, cObject);
    cArray = rb_define_class("Array", cObject, cObject);
    cHash = rb_define_class("Hash", cObject, cObject);
    cRange = rb_define_class("Range", cObject, cObject);
    cSymbol = rb_define_class("Symbol", cObject, cObject);
    cTrueClass = rb_define_class("TrueClass", cObject, cObject);
    cFalseClass = rb_define_class("FalseClass", cObject, cObject);
    cNilClass = rb_define_class("NilClass", cObject, cObject);
    cProc = rb_define_class("Proc", cObject, cObject);

    cException = rb_define_class("Exception", cObject, cObject);
    cStandardError = rb_define_class("StandardError", cException, cObject);
    cRuntimeError = rb_define_class("RuntimeError", cStandardError, cObject);
    cTypeError = rb_define_class("TypeError", cStandardError, cObject);
    cArgumentError = rb_define_class("ArgumentError", cStandardError, cObject);
    cNoMethodError = rb_define_class("NoMethodError", cStandardError, cObject);
    cZeroDivisionError = rb_define_class("ZeroDivisionError", cStandardError, cObject);
    cIndexError = rb_define_class("IndexError", cStandardError, cObject);
    cNameError = rb_define_class("NameError", cStandardError, cObject);

    mMath = rb_define_module("Math", cObject);
    rb_const_set(mMath, "PI", rb_float_new(3.14159265358979323846));
    rb_const_set(mMath, "E", rb_float_new(2.71828182845904523536));
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "sqrt", m_math_sqrt, 1);
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "sin", m_math_sin, 1);
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "cos", m_math_cos, 1);
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "tan", m_math_tan, 1);
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "log", m_math_log, 1);
    rb_def_singleton_method((RValue)(uintptr_t)mMath, "exp", m_math_exp, 1);

    cFile = rb_define_class("File", cObject, cObject);
    rb_def_singleton_method((RValue)(uintptr_t)cFile, "read", m_file_read, 1);
    rb_def_singleton_method((RValue)(uintptr_t)cFile, "write", m_file_write, 2);
    rb_def_singleton_method((RValue)(uintptr_t)cFile, "exist?", m_file_exist, 1);
    rb_def_singleton_method((RValue)(uintptr_t)cFile, "exists?", m_file_exist, 1);

    /* Kernel / Object methods */
    rb_def_method(mKernel, "puts", m_puts, -1);
    rb_def_method(mKernel, "print", m_print, -1);
    rb_def_method(mKernel, "p", m_p, -1);
    rb_def_method(mKernel, "gets", m_gets, 0);
    rb_def_method(mKernel, "raise", m_raise, -1);
    rb_def_method(mKernel, "exit", m_exit, -1);
    rb_def_method(mKernel, "rand", m_rand, -1);
    rb_def_method(mKernel, "srand", m_srand, -1);
    rb_def_method(mKernel, "Integer", m_conv_int, 1);
    rb_def_method(mKernel, "Float", m_conv_float, 1);
    rb_def_method(mKernel, "String", m_conv_str, 1);
    rb_def_method(mKernel, "Array", m_conv_ary, 1);

    rb_def_method(cObject, "class", m_obj_class, 0);
    rb_def_method(cObject, "is_a?", m_obj_is_a, 1);
    rb_def_method(cObject, "kind_of?", m_obj_is_a, 1);
    rb_def_method(cObject, "respond_to?", m_obj_respond_to, 1);
    rb_def_method(cObject, "send", m_obj_send, -1);
    rb_def_method(cObject, "nil?", m_obj_nil_p, 0);
    rb_def_method(cObject, "to_s", m_obj_to_s, 0);
    rb_def_method(cObject, "inspect", m_obj_inspect, 0);
    rb_def_method(cObject, "==", m_obj_eq, 1);
    rb_def_method(cObject, "!=", m_obj_neq, 1);
    rb_def_method(cObject, "===", m_obj_eq, 1);
    rb_def_method(cObject, "equal?", m_obj_eq, 1);
    rb_def_method(cObject, "object_id", m_obj_id, 0);

    /* Module / Class */
    rb_def_method(cModule, "name", m_class_name, 0);
    rb_def_method(cModule, "to_s", m_class_name, 0);
    rb_def_method(cModule, "inspect", m_class_name, 0);
    rb_def_method(cModule, "===", m_class_case_eq, 1);
    rb_def_method(cClass, "new", m_class_new, -1);

    /* Integer */
    rb_def_method(cInteger, "times", m_int_times, 0);
    rb_def_method(cInteger, "upto", m_int_upto, 1);
    rb_def_method(cInteger, "downto", m_int_downto, 1);
    rb_def_method(cInteger, "abs", m_int_abs, 0);
    rb_def_method(cInteger, "even?", m_int_even, 0);
    rb_def_method(cInteger, "odd?", m_int_odd, 0);
    rb_def_method(cInteger, "zero?", m_int_zero, 0);
    rb_def_method(cInteger, "to_i", m_int_to_i, 0);
    rb_def_method(cInteger, "to_f", m_int_to_f, 0);
    rb_def_method(cInteger, "to_s", m_int_to_s, 0);
    rb_def_method(cInteger, "succ", m_int_succ, 0);
    rb_def_method(cInteger, "next", m_int_succ, 0);
    rb_def_method(cInteger, "pred", m_int_pred, 0);
    rb_def_method(cInteger, "chr", m_int_chr, 0);

    /* Float */
    rb_def_method(cFloat, "abs", m_flt_abs, 0);
    rb_def_method(cFloat, "round", m_flt_round, -1);
    rb_def_method(cFloat, "floor", m_flt_floor, 0);
    rb_def_method(cFloat, "ceil", m_flt_ceil, 0);
    rb_def_method(cFloat, "to_i", m_flt_to_i, 0);
    rb_def_method(cFloat, "to_f", m_flt_to_f, 0);
    rb_def_method(cFloat, "to_s", m_flt_to_s, 0);

    /* String */
    rb_def_method(cString, "+", m_str_add, 1);
    rb_def_method(cString, "*", m_str_mul, 1);
    rb_def_method(cString, "<<", m_str_shl, 1);
    rb_def_method(cString, "==", m_str_eq, 1);
    rb_def_method(cString, "<=>", m_str_cmp, 1);
    rb_def_method(cString, "<", m_str_lt, 1);
    rb_def_method(cString, "<=", m_str_le, 1);
    rb_def_method(cString, ">", m_str_gt, 1);
    rb_def_method(cString, ">=", m_str_ge, 1);
    rb_def_method(cString, "length", m_str_len, 0);
    rb_def_method(cString, "size", m_str_len, 0);
    rb_def_method(cString, "empty?", m_str_empty, 0);
    rb_def_method(cString, "[]", m_str_aref, -1);
    rb_def_method(cString, "upcase", m_str_upcase, 0);
    rb_def_method(cString, "downcase", m_str_downcase, 0);
    rb_def_method(cString, "reverse", m_str_reverse, 0);
    rb_def_method(cString, "strip", m_str_strip, 0);
    rb_def_method(cString, "chomp", m_str_chomp, 0);
    rb_def_method(cString, "split", m_str_split, -1);
    rb_def_method(cString, "chars", m_str_chars, 0);
    rb_def_method(cString, "include?", m_str_include, 1);
    rb_def_method(cString, "start_with?", m_str_start_with, 1);
    rb_def_method(cString, "end_with?", m_str_end_with, 1);
    rb_def_method(cString, "to_i", m_str_to_i, -1);
    rb_def_method(cString, "to_f", m_str_to_f, 0);
    rb_def_method(cString, "to_s", m_str_to_s, 0);
    rb_def_method(cString, "to_sym", m_str_to_sym, 0);
    rb_def_method(cString, "inspect", m_str_inspect, 0);
    rb_def_method(cString, "ord", m_str_ord, 0);
    rb_def_method(cString, "gsub", m_str_gsub, 2);

    /* Array */
    rb_def_method(cArray, "[]", m_ary_aref, -1);
    rb_def_method(cArray, "[]=", m_ary_aset, 2);
    rb_def_method(cArray, "length", m_ary_len, 0);
    rb_def_method(cArray, "size", m_ary_len, 0);
    rb_def_method(cArray, "empty?", m_ary_empty, 0);
    rb_def_method(cArray, "push", m_ary_push, -1);
    rb_def_method(cArray, "<<", m_ary_push, 1);
    rb_def_method(cArray, "pop", m_ary_pop, 0);
    rb_def_method(cArray, "shift", m_ary_shift, 0);
    rb_def_method(cArray, "unshift", m_ary_unshift, -1);
    rb_def_method(cArray, "first", m_ary_first, 0);
    rb_def_method(cArray, "last", m_ary_last, 0);
    rb_def_method(cArray, "include?", m_ary_include, 1);
    rb_def_method(cArray, "index", m_ary_index, 1);
    rb_def_method(cArray, "each", m_ary_each, 0);
    rb_def_method(cArray, "each_with_index", m_ary_each_with_index, 0);
    rb_def_method(cArray, "map", m_ary_map, 0);
    rb_def_method(cArray, "collect", m_ary_map, 0);
    rb_def_method(cArray, "select", m_ary_select, 0);
    rb_def_method(cArray, "filter", m_ary_select, 0);
    rb_def_method(cArray, "reject", m_ary_reject, 0);
    rb_def_method(cArray, "reduce", m_ary_reduce, -1);
    rb_def_method(cArray, "inject", m_ary_reduce, -1);
    rb_def_method(cArray, "any?", m_ary_any, 0);
    rb_def_method(cArray, "all?", m_ary_all, 0);
    rb_def_method(cArray, "find", m_ary_find, 0);
    rb_def_method(cArray, "detect", m_ary_find, 0);
    rb_def_method(cArray, "sum", m_ary_sum, -1);
    rb_def_method(cArray, "min", m_ary_min, 0);
    rb_def_method(cArray, "max", m_ary_max, 0);
    rb_def_method(cArray, "reverse", m_ary_reverse, 0);
    rb_def_method(cArray, "sort", m_ary_sort, 0);
    rb_def_method(cArray, "uniq", m_ary_uniq, 0);
    rb_def_method(cArray, "compact", m_ary_compact, 0);
    rb_def_method(cArray, "join", m_ary_join, -1);
    rb_def_method(cArray, "+", m_ary_add, 1);
    rb_def_method(cArray, "==", m_ary_eq, 1);
    rb_def_method(cArray, "to_s", m_ary_inspect, 0);
    rb_def_method(cArray, "inspect", m_ary_inspect, 0);

    /* Hash */
    rb_def_method(cHash, "[]", m_hash_aref, 1);
    rb_def_method(cHash, "[]=", m_hash_aset, 2);
    rb_def_method(cHash, "length", m_hash_len, 0);
    rb_def_method(cHash, "size", m_hash_len, 0);
    rb_def_method(cHash, "empty?", m_hash_empty, 0);
    rb_def_method(cHash, "keys", m_hash_keys, 0);
    rb_def_method(cHash, "values", m_hash_values, 0);
    rb_def_method(cHash, "has_key?", m_hash_has_key, 1);
    rb_def_method(cHash, "key?", m_hash_has_key, 1);
    rb_def_method(cHash, "include?", m_hash_has_key, 1);
    rb_def_method(cHash, "delete", m_hash_delete, 1);
    rb_def_method(cHash, "each", m_hash_each, 0);
    rb_def_method(cHash, "to_s", m_hash_inspect, 0);
    rb_def_method(cHash, "inspect", m_hash_inspect, 0);

    /* Range */
    rb_def_method(cRange, "each", m_range_each, 0);
    rb_def_method(cRange, "to_a", m_range_to_a, 0);
    rb_def_method(cRange, "map", m_range_map, 0);
    rb_def_method(cRange, "include?", m_range_include, 1);
    rb_def_method(cRange, "===", m_range_include, 1);

    /* Proc */
    rb_def_method(cProc, "call", m_proc_call, -1);
    rb_def_method(cProc, "[]", m_proc_call, -1);

    /* Symbol */
    rb_def_method(cSymbol, "to_s", m_sym_to_s, 0);
    rb_def_method(cSymbol, "id2name", m_sym_to_s, 0);

    /* Exception */
    rb_def_method(cException, "message", m_exc_message, 0);
    rb_def_method(cException, "to_s", m_exc_message, 0);

    /* Top-level self & ARGV */
    RInstance* top = (RInstance*)rb_alloc_obj(OBJ_INSTANCE, sizeof(RInstance));
    top->klass = cObject;
    g_top_self = (RValue)(uintptr_t)top;

    RValue argv_ary = rb_ary_new(0, NULL);
    for (int i = 1; i < argc; i++) {
        rb_ary_push(argv_ary, rb_str_new_cstr(argv[i]));
    }
    rb_const_set(cObject, "ARGV", argv_ary);
}

void rb_shutdown(void) {
    /* Cleanup handled by OS on exit */
}
