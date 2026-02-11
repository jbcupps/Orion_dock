# Soul Forge Protocol

The **Soul Forge** is a psychometric calibration ritual that replaces the standard configuration wizard. It determines the agent's personality (Deontology, Teleology, Areteology, Welfare) and generates a unique "Soul Sigil".

## Usage

Run the forge to initialize your agent's soul:

```bash
# Linux/Mac
./scripts/soul-forge.sh

# Windows
./scripts/soul-forge.ps1
```

Or directly via cargo:

```bash
cargo run -p soul-forge
```

**Requirements:** Rust toolchain; on Windows, Visual Studio Build Tools with the C++ workload (for `msvcrt.lib` and the linker).

## Output

The forge generates two files in your `docs` directory:

1.  `soul.md`: The System Prompt context for the LLM.
2.  `soul.json`: The consensus record containing the weights and hash.

It also completes the "Birth" process, signing the constitutional documents.
