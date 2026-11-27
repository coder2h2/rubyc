# rubyc

An Ahead-Of-Time (AOT) Ruby compiler written in Rust that compiles Ruby source code directly into native **x86-64** (System V ABI) or **ARM64** (AAPCS64) assembly and links it with an embedded C runtime (`librubycom_rt.a`) to produce standalone native ELF64 executables.

## Installation

### From `.deb` (Debian / Ubuntu — `amd64` & `arm64`)

Download the `.deb` for your architecture from the [GitHub Releases](https://github.com/coder2h2/rubyc/releases) page and install it:

```bash
sudo apt install ./rubyc_0.1.0-1_amd64.deb   # or arm64
```

### From Source

Requires `rustc`, `cargo`, `gcc`, and `binutils` (`as`):

```bash
cargo install --path .
```

## Usage

Compile a Ruby script to a native executable:

```bash
rubyc hello.rb
./hello
```

Options:
- `-o <output>` — Specify the output binary (or `.s` file with `--emit-asm`)
- `--target x86_64|aarch64` — Select target architecture (defaults to host architecture)
- `--emit-asm`, `-S` — Emit assembly without assembling or linking
- `--run`, `-r` — Execute the compiled binary immediately after linking (`-- <args>...` forwards arguments)

## Supported Ruby Features

- **Types**: 63-bit tagged Fixnum `Integer`s, `Float`s, `String`s (single/double-quoted with `#{}` interpolation), `Symbol`s, `Array`s, `Hash`es, `Range`s (`..` / `...`), `true`, `false`, `nil`
- **Control Flow**: `if`/`elsif`/`else`/`unless`, postfix modifiers (`if`/`unless`/`while`/`until`), ternary `?:`, `while`/`until`/`for`, `break`/`next`, `case`/`when`
- **Functions & Closures**: `def` (default args, `*splat`, `&block`, singleton methods), `return`, `yield`, blocks (`do..end` / `{..}`) and lambdas (`->`, `lambda`) with mutable lexical variable capture
- **OOP**: `class` inheritance, `module` & `include`, `super`, `attr_reader`/`attr_writer`/`attr_accessor`, `@ivar`, `@@cvar`, `$gvar`, constants
- **Exceptions**: `begin`/`rescue`/`ensure`/`raise` via `setjmp`/`longjmp`, safe navigation (`&.`)
- **Built-in Runtime**: Conservative mark-and-sweep GC and core methods on `Kernel`/`Object`, `Integer`, `Float`, `Math`, `String`, `Array`, `Hash`, `Range`, `Proc`, `Symbol`, `Exception`, and `File`

## Testing

Run the differential test suite against MRI `ruby`:

```bash
cargo build
bash tests/run_tests.sh
```

## License

MIT — Gideon Laurie `<gideon.a.e.laurie@gmail.com>`
