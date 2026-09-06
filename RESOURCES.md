# RESOURCES — Rust Ownership / Borrowing / Move Semantics

> All URLs verified live on 2026-08-04 (fetch or GitHub API), except where noted.
> Learner context: Rust edition 2024, axum 0.8 + tokio + sqlx knowledge-base web service.

## Knowledge (入门讲解 → 深入原理)

- **The Rust Programming Language ("the Book") — Ch.4 Understanding Ownership**
  - https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html (chapter overview)
  - https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html (ownership rules, move/clone/Copy)
  - https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html (references & borrowing)
  - Use for: THE canonical first read on ownership, borrowing, and move; the authoritative baseline for the teaching notes.

- **Rust 程序设计语言（the Book 简体中文版）— 第 4 章 认识所有权**
  - https://kaisery.github.io/trpl-zh-cn/ch04-00-understanding-ownership.html
  - Use for: Chinese-native first pass; translation tracks Rust 1.90.0 / edition 2024 (matches the learner's setup).

- **Interactive Rust Book (Brown University Cognitive Engineering Lab)**
  - https://rust-book.cs.brown.edu/
  - Use for: Research-backed interactive fork of the Book — quizzes + Aquascope step-by-step memory visualizations; its ownership chapter is specifically redesigned for better understanding.

- **Rust By Example — Scoping rules / Ownership and moves**
  - https://doc.rust-lang.org/rust-by-example/scope.html (chapter: RAII, ownership/move, borrow, lifetimes)
  - https://doc.rust-lang.org/rust-by-example/scope/move.html (ownership & move, with runnable snippets)
  - Use for: Minimal runnable examples to contrast Copy vs move vs borrow right after reading the Book.

- **通过例子学 Rust（Rust By Example 中文版）— 所有权和移动**
  - https://rustwiki.org/zh-CN/rust-by-example/scope/move.html
  - Use for: Chinese mirror of the same scope/ownership/borrow/lifetime chapter.

- **Comprehensive Rust (Google Android team)**
  - https://google.github.io/comprehensive-rust/
  - https://google.github.io/comprehensive-rust/memory-management.html (Ownership, Move Semantics, Clone, Copy, Drop + Builder exercise)
  - https://google.github.io/comprehensive-rust/memory-management/ownership.html
  - Use for: Slide-format course with speaker notes and a Simplified Chinese translation; good second explanation with timed lesson structure.

- **The Rust Reference — ownership-adjacent normative semantics**
  - https://doc.rust-lang.org/reference/
  - https://doc.rust-lang.org/reference/destructors.html (drop scopes, drop order, temporary scopes & lifetime extension)
  - Use for: Precise drop/temporary semantics when the Book is vague. Note: the Reference has NO ownership/borrow-checker chapter — do not use it to learn ownership itself.

- **The Rustonomicon**
  - https://doc.rust-lang.org/nomicon/
  - Use for: Deep-dive later stage only (unsafe Rust, variance/subtyping, drop check); explicitly not a beginner resource.

- **Rust for Rustaceans — Jon Gjengset (No Starch Press)**
  - https://rust-for-rustaceans.com/ (official site; publisher link nostarch.com/rust-rustaceans on the page)
  - Use for: Intermediate→advanced book; ch.1 re-derives ownership/lifetimes/variance for professional use. Paid; read after the Book.

- **Crust of Rust: Lifetime Annotations — Jon Gjengset (video)**
  - https://www.youtube.com/watch?v=rAl-9HwD858 (~1.5h live-coding session)
  - Series playlist: https://www.youtube.com/playlist?list=PLqbS7AVVErFiWDOAVrPt7aYmnuuOLYvOa
  - Use for: The natural next step after ownership — why/how lifetime annotations work, by example in real code.

## Practice (刻意练习)

- **Rustlings — move semantics module (ownership-specific)**
  - https://github.com/rust-lang/rustlings
  - Module: exercises/06_move_semantics/ → move_semantics1.rs … move_semantics5.rs
    https://github.com/rust-lang/rustlings/tree/main/exercises/06_move_semantics
  - Use for: THE targeted ownership drill — fix-the-compile-error exercises on moves; note the old "moves1/moves2" names are now move_semantics1..5 in the current layout.

- **Rustlings — lifetimes module (borrowing-specific)**
  - https://github.com/rust-lang/rustlings/tree/main/exercises/16_lifetimes
  - Use for: Follow-up drill once borrowing/lifetime errors appear in the learner's axum/sqlx code.

- **Exercism Rust Track**
  - https://exercism.org/tracks/rust
  - Use for: 99 exercises with automatic analysis + free human mentoring (1291 mentors); ownership pressure shows up naturally in real exercises.

- **Rust Playground**
  - https://play.rust-lang.org/
  - Use for: Zero-setup scratchpad to test borrow-checker hypotheses while reading (linked from rust-lang.org official nav).

## Wisdom (社区 / Communities)

- **The Rust Users Forum (official)**
  - https://users.rust-lang.org/
  - Use for: Primary Q&A venue; verified active 2026-08-04 (daily borrow-checker/help threads). English.

- **Rust 语言中文社区 (rustcc.cn)**
  - https://rustcc.cn/
  - Use for: Most active Chinese Rust forum — verified active 2026-08-04 (daily Rust日报, Q&A 版块). First stop for Chinese-language questions.
  - WARNING: the old domain rust.cc is DEAD (serves an OpenResty default page). Do not link it.

- **r/rust (Reddit)**
  - https://www.reddit.com/r/rust/
  - Use for: News, ecosystem discussion, searchable past threads. Note: Reddit blocks automated fetchers, so the page itself could not be machine-verified; URL is canonical and activity is evidenced weekly via This Week in Rust.

- **Rust Zulip (official chat)**
  - https://rust-lang.zulipchat.com/
  - Use for: Official chat platform per rust-lang.org/community; where language teams discuss design (e.g. t-lang streams). Good for advanced "why does the language do this" questions.

- **Rust Programming Language Community Discord (community-run)**
  - https://discord.com/invite/rust-lang-community
  - Use for: Active real-time help channels (#help etc.). IMPORTANT: the official Rust Discord was shut down and is read-only (https://forge.rust-lang.org/platforms/discord.html); discord.gg/rust-lang is only an archive.

- **This Week in Rust (newsletter)**
  - https://this-week-in-rust.org/
  - Use for: Weekly digest of community/RFCs/news (issue #662 on 2026-07-29); keeps the learner plugged into the ecosystem.

- **Official community page (entry point)**
  - https://www.rust-lang.org/community
  - Use for: Authoritative list of official channels (Users Forum, Internals Forum, Zulip).

## Verification notes (2026-08-04)

- All doc.rust-lang.org, google.github.io, exercism.org, rustcc.cn, users.rust-lang.org, zulipchat, rust-for-rustaceans.com, kaisery.github.io, rustwiki.org, rust-book.cs.brown.edu, this-week-in-rust.org, forge.rust-lang.org URLs fetched successfully.
- Rustlings module/file names confirmed via GitHub contents API on main branch (06_move_semantics: move_semantics1-5.rs; 16_lifetimes exists).
- nostarch.com/rust-rustaceans returns 403 to bots; the book is cited via the author's official site instead.
- reddit.com blocks automated fetches (transport error on www/old/RSS); included as canonical URL with caveat.
- rust.cc: dead. Official Rust Discord: shut down (read-only archive). Both replaced with live alternatives above.
