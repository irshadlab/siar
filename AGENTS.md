# SIAR Agent Guidelines — Code Intelligence (Octocode & XERJ)

When the user asks questions about anything in this codebase (architecture, crates, modules, functions, types, traits, bugs, dependencies, or workflows), ground answers using **XERJ** and **Octocode** MCP tools (`call_mcp_tool`).

## Tool Specialization: When to Use Which

| Task | Tool to Use | ServerName | Key Tools |
| :--- | :--- | :--- | :--- |
| **Fast AST & Full-Text Search** | **XERJ** | `"xerj"` | `xerj_code_search`, `xerj_search`, `xerj_def` |
| **Workspace & Dataset Mapping** | **XERJ** | `"xerj"` | `xerj_map` |
| **Persistent Agent Memory** | **XERJ** | `"xerj"` | `xerj_memory_store`, `xerj_memory_recall` |
| **Compiler / LSP Semantics** | **Octocode** | `"octocode"` | `lspGetSemantics` (`callers`, `callees`, type definitions, implementations) |
| **Fast File Content / Windows** | **Octocode** / Read | `"octocode"` | `localGetFileContent` |
| **Upstream & External Code** | **Octocode** | `"octocode"` | `ghSearchCode`, `ghGetFileContent` (OpenMLS, Slint, Ring, Tokio) |

---

## Research Loop
Follow the evidence-first loop: `SCOPE → ORIENT → SEARCH → READ EXACT → PROVE → ANSWER`

1. **Orient & Inspect Structure**
   - Use `xerj_map` (or `localSearch` with `operation: "tree"` / `"files"`) to view crate structure without massive directory dumps.
   - Use `localAnalyzeGraph` to map dependency graphs and module boundaries.

2. **Search (Fast Candidate Discovery)**
   - Use `xerj_code_search` or `xerj_search` for fast AST/hybrid symbol lookup.
   - Or use `localSearch` (`operation: "text"` or `"structural"`) scoped to specific crates (e.g. `crates/siar-identity-multidevice`).

3. **Read Exact Implementation**
   - Use `localGetFileContent` with targeted line ranges or match windows to inspect the exact bytes without bloating the context window.

4. **Prove via LSP (Rust-Analyzer)**
   - Use `lspGetSemantics` from Octocode for compiler-verified symbol definitions, call hierarchies (`callers`/`callees`), implementations, type definitions, and reference tracking.

5. **External & Upstream Code**
   - For upstream dependencies (e.g. OpenMLS, Slint, Tokio, Ring), use `ghSearchCode` and `ghGetFileContent` to inspect upstream code.

## Response Guidelines
- Ground all explanations in fetched code bytes, file paths, and exact line numbers.
- Provide clickable markdown file links (e.g., `[filename.rs](file:///home/irshad/Projects/siar/crates/.../filename.rs#L10-L30)`).
