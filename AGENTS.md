- Avoid writing overly defensive code.
- Write framework-quality code with clear abstractions, consistent conventions, and well-defined responsibilities, without overengineering.
- Embrace **Clean Code** and **Software Craftsmanship**. Think from first principles and treat code as a work of art. Prioritize correctness, security, performance, and readability while striving for simplicity, clarity, consistency, and elegance. Minimize necessary complexity and maintenance burden. Every abstraction must justify its existence, every module must have a clear responsibility, and every line of code must serve a purpose. Favor thoughtful design over cleverness, and essential simplicity over superficial brevity.
- Before adding complexity for a speculative or vanishingly unlikely race or security edge case, explain the concrete failure mode, likelihood, and complexity cost to the user and get their buy-in. Do not silently expand scope for theoretical robustness.
- Test only complex or high-risk changes, or when explicitly requested. Batch focused test runs at a meaningful milestone; rerun only to investigate a failure or verify its fix.
- The primary agent owns architecture, planning, task decomposition, test strategy, scenario design, scope, and acceptance criteria. Do not delegate planning or final decisions.
- Delegate scoped implementation tasks to `worker` subagents when beneficial. Workers may modify production code within their assigned scope. Avoid overlapping file ownership across concurrent workers.
- Delegate code reviews to `review` subagents when beneficial. Reviewers must not modify files; they only identify concrete issues and report actionable findings.
- `worker` and `review` subagents must inherit the primary agent's model.
- Delegate complex test implementation, execution, and evidence collection to a **GPT-6 Luna** subagent with maximum reasoning effort. Keep straightforward test work with the primary agent.
- The GPT-6 Luna testing subagent must never modify production source code or implementation files. Its write access is strictly limited to test files. It may implement and execute tests, collect evidence, and report defects, but must leave all production fixes and implementation changes to the primary agent or its workers.
- The primary agent is responsible for integrating delegated work, resolving review findings, and verifying the final result.

## Lint baseline

`cargo xtask lint` is the only entry point and the commit gate: rustfmt + `cargo clippy
--workspace --all-targets` (QuickJS, all built-in modules) + clippy over the same code under
the V8 backend.
