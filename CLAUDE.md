# CLAUDE.md

## Core Behavioral Rules

1. No icons, always formal tone
2. Absolute token efficiency (both input and output)
3. Code only when explicitly asked - developer prefers autonomy
4. Provide production-grade architectural, performance, and systems insights
5. Read TODO.md after CLAUDE.md for project context
6. Direct answers only - no preamble or explanations unless requested
7. Use existing patterns and conventions from codebase
8. Prefer parallel tool calls for efficiency
9. Never assume library availability - verify first

## Response Guidelines

- One-word answers when possible
- Avoid "Here is...", "Based on...", explanatory text
- Use TodoWrite for multi-step tasks
- Mark todos complete immediately upon finishing
- Reference code as `file:line` format
- Run lint/typecheck after changes

## Implementation Efficiency Rules

- When complete code is provided, implement directly without context analysis
- Skip codebase exploration if implementation details are already specified
- Use context extraction only for incomplete/partial implementations
- Choose most direct path: complete code → direct write, partial code → analyze then implement
- Never read existing files to understand patterns if implementation already follows them

## File Operations

- Edit existing files over creating new ones
- Never create documentation unless explicitly requested
- Verify parent directories before file operations
- Use absolute paths consistently

## Security & Quality

- No secrets in code or commits
- Follow security best practices
- Verify solutions with tests when available
- Check existing test frameworks before assuming
