# CLAUDE.md - Roughup Quality Protocol

## Core Rule: Quality Over Velocity
User is permanent project memory. Always ask "why" and "where" before implementing.

## Pre-Implementation Protocol
Before any struct/enum/function/trait:
1. **"Why was [related component] implemented this way?"**
2. **"Where should I examine for context?"** (never blind search)
3. **"How does this affect our SLAs?"** (<2s context, <300ms rollback)

## Critical Constraints
- **Performance**: Respect existing SLAs, discuss hot-path allocations
- **Safety**: Preserve atomic guarantees, BLAKE3 checksums, repo boundaries
- **Architecture**: Align with Phase 3.5 (conflict resolution), Priority/SymbolRanker systems

## Mandatory Development Workflow
**Context Assembly**: Before implementing any feature, ALWAYS use `rup context` with proper budgeting to gather relevant context. This serves dual purposes:
- **Quality**: Ensures implementation aligns with existing patterns and architecture
- **Validation**: Tests our flagship context assembly functionality during development

Example: `rup context --budget 4000 --template feature "resolve" "conflict" "SmartMerge" --semantic`

## Session Enforcement
**Start**: Acknowledge CLAUDE.md + TODO.md + **lib.rs** (mandatory full read), confirm current phase
- **lib.rs**: Project architecture blueprint - explains modules, performance targets, strategic re-exports
- **Use lib.rs**: To identify `rup context` keywords and understand module relationships before implementing
**Mid-session** (every 5-7 exchanges): "Am I maintaining quality-first approach?"
**Rule violation**: User says "PAUSE" → re-read rules, restart with discovery questions

## Implementation Efficiency
- Complete specs → implement directly
- Partial specs → ask "Where should I examine patterns?"
- All code → compact inline comments, `file:line` references

## Insights Format
```
★ Insight ─────────────────────────────────────
[2-3 key points specific to implementation]
─────────────────────────────────────────────────
```