# Changelog

## [1.0.4](https://github.com/Michael-JB/ctx/compare/v1.0.3...v1.0.4) (2026-08-27)


### Bug Fixes

* launcher panes take keyboard input again ([7f63bc3](https://github.com/Michael-JB/ctx/commit/7f63bc3b65c04f2a64b5eef1d4832ae64a475f50))

## [1.0.3](https://github.com/Michael-JB/ctx/compare/v1.0.2...v1.0.3) (2026-08-27)


### Bug Fixes

* pane commands run in the user's shell environment again ([1b98aeb](https://github.com/Michael-JB/ctx/commit/1b98aeb1cefde89ab3ae9c56a67ab953974c575d))

## [1.0.2](https://github.com/Michael-JB/ctx/compare/v1.0.1...v1.0.2) (2026-08-21)


### Bug Fixes

* ctx new --detach inside zellij no longer opens tabs in the current session ([4c236a4](https://github.com/Michael-JB/ctx/commit/4c236a44b51c1f271acd83373e67e4e667e160cd))

## [1.0.1](https://github.com/Michael-JB/ctx/compare/v1.0.0...v1.0.1) (2026-08-21)


### Bug Fixes

* a delete or archive that fails no longer leaves the session running ([083301e](https://github.com/Michael-JB/ctx/commit/083301ebe867b6f53b1e615ee88f4aab5e75ff3d))
* deleting or archiving the current context no longer leaves the checkout behind ([a61771c](https://github.com/Michael-JB/ctx/commit/a61771cd25498b309bdb4d1f61854c6132d6b1e3))

## [1.0.0](https://github.com/Michael-JB/ctx/compare/v0.9.1...v1.0.0) (2026-08-21)


### ⚠ BREAKING CHANGES

* ctx claude-hook is gone. Update Claude Code hook configs to invoke ctx builtin claude status-hook instead.

### Features

* move builtin entry points under a hidden ctx builtin group ([e681421](https://github.com/Michael-JB/ctx/commit/e6814212762e41b157208e800c29c8e5aec597a0))
* pre-answer Claude Code's trust dialog for context checkouts ([299b9d9](https://github.com/Michael-JB/ctx/commit/299b9d919ff36047241553fa230002d75cade278))

## [0.9.1](https://github.com/Michael-JB/ctx/compare/v0.9.0...v0.9.1) (2026-08-20)


### Bug Fixes

* tmux no longer truncates long pane commands ([56c5830](https://github.com/Michael-JB/ctx/commit/56c58302fa0ea99899aec2058ef92921398b1cb1))

## [0.9.0](https://github.com/Michael-JB/ctx/compare/v0.8.0...v0.9.0) (2026-08-20)


### Features

* hand a new context's Claude an initial prompt via ctx new --set ([c15a51c](https://github.com/Michael-JB/ctx/commit/c15a51c66cfb85cdc46d42a6b86a929780ad5b6e))
* make agents ctx-aware via ctx agent-docs ([138e477](https://github.com/Michael-JB/ctx/commit/138e4773d960bb2269ed328213da73aff22894a1))
* print the installed version's changelog via ctx changelog ([a886018](https://github.com/Michael-JB/ctx/commit/a8860188e77d206f362a1e07088bcc5c5b6568b0))
* resume the Claude conversation when a session is recreated ([8679f19](https://github.com/Michael-JB/ctx/commit/8679f19528fd8f71cefb5ff5783707746029078a))
* run Claude Code in a pane via the claude layout builtin ([effcea7](https://github.com/Michael-JB/ctx/commit/effcea74b1b102b33711990f1360ea7a64f398d2))
* start a context's session without attaching via ctx new --detach ([cc47888](https://github.com/Michael-JB/ctx/commit/cc47888a6e86d52b1968a705594693b74f3ce29c))


### Bug Fixes

* escape quotes and newlines in zellij layout strings ([d41dec4](https://github.com/Michael-JB/ctx/commit/d41dec4979f5addec4a42bfdd75a89d0e89d7247))
* leave no broken context behind when a deletion is interrupted ([92abe66](https://github.com/Michael-JB/ctx/commit/92abe6673b8ed7c92eee22b262d0b121488a5727))
* list a damaged checkout as branchless instead of crashing ([b1ab1f9](https://github.com/Michael-JB/ctx/commit/b1ab1f99f5f3ae503211a8f2a698ec4ca04fb92a))
* **tui:** finish interrupted deletions at startup ([df21866](https://github.com/Michael-JB/ctx/commit/df21866ebb0a1c142d1b79677879a8a8e312dbe8))
* **tui:** kill a context's session even when its removal is interrupted ([62d1119](https://github.com/Michael-JB/ctx/commit/62d11196ac8dc39f1f6381448dd61ded95e1d79c))


### Upgrade Notes

* ensure layout panes running Claude use builtin = "claude" rather than a command ([effcea7](https://github.com/Michael-JB/ctx/commit/effcea74b1b102b33711990f1360ea7a64f398d2))
* ensure the agent docs are installed as a skill (README &gt; Agent docs) ([138e477](https://github.com/Michael-JB/ctx/commit/138e4773d960bb2269ed328213da73aff22894a1))

## [0.8.0](https://github.com/Michael-JB/ctx/compare/v0.7.2...v0.8.0) (2026-08-18)


### Features

* default new-context names to a random adjective-animal pair ([7fcc091](https://github.com/Michael-JB/ctx/commit/7fcc0911e5bedc0b0776be1c89ff022861154178))


### Bug Fixes

* **tui:** style dialog inputs with the configured theme ([9f1d9dd](https://github.com/Michael-JB/ctx/commit/9f1d9dd17dfce573e82a960040f7c653e804ca3f))

## [0.7.2](https://github.com/Michael-JB/ctx/compare/v0.7.1...v0.7.2) (2026-08-17)


### Bug Fixes

* adopt an existing local branch instead of failing to fork it ([51ab9ed](https://github.com/Michael-JB/ctx/commit/51ab9ed30dd78fd6f9173d5dc657606208717210))

## [0.7.1](https://github.com/Michael-JB/ctx/compare/v0.7.0...v0.7.1) (2026-08-17)


### Bug Fixes

* create contexts even when run from a deleted directory ([79025fa](https://github.com/Michael-JB/ctx/commit/79025fa1defed8042e1cfb24738f9e6a52d37a31))

## [0.7.0](https://github.com/Michael-JB/ctx/compare/v0.6.2...v0.7.0) (2026-08-14)


### Features

* shift+D permanently deletes a context from the contexts panel ([6eb2a44](https://github.com/Michael-JB/ctx/commit/6eb2a4418427a173d4578856e1e382cfb1171b0a))

## [0.6.2](https://github.com/Michael-JB/ctx/compare/v0.6.1...v0.6.2) (2026-08-14)


### Bug Fixes

* fetch LFS objects into repo mirrors so clones can smudge locally ([1de48a1](https://github.com/Michael-JB/ctx/commit/1de48a1994426e79f70e85f89d59d5c4b215daf8))
* remove the partial checkout left behind by a failed clone ([9d45c37](https://github.com/Michael-JB/ctx/commit/9d45c37a947e8d4021ca2669aa6e32e5bcd027a3))
* surface git stderr in error messages instead of just the exit status ([be74f50](https://github.com/Michael-JB/ctx/commit/be74f506c37eeed6b86e1e25f61aa683720bd039))
* unstick the TUI after archiving the context it is running in ([49dceeb](https://github.com/Michael-JB/ctx/commit/49dceeb0c76b595b2e06c67232b7c654eba8e23f))

## [0.6.1](https://github.com/Michael-JB/ctx/compare/v0.6.0...v0.6.1) (2026-08-13)


### Performance Improvements

* paint the empty panels before the first data load ([6b5ad4f](https://github.com/Michael-JB/ctx/commit/6b5ad4fa1f4763915c6ccad8e7d7e10dd8c73688))
* read the current branch from .git/HEAD instead of spawning git ([4bf23d0](https://github.com/Michael-JB/ctx/commit/4bf23d00b5e43602f752385f267c1df43efc4169))

## [0.6.0](https://github.com/Michael-JB/ctx/compare/v0.5.0...v0.6.0) (2026-08-13)


### Features

* agent column shows how long active states have run ([40d9c80](https://github.com/Michael-JB/ctx/commit/40d9c8073067d44bff56da482150bf4ec7c6ab07))
* builtin status cells render as coloured nerd-font glyphs ([f8767cf](https://github.com/Michael-JB/ctx/commit/f8767cf5e1f75e0150e10944c318479efba10dd7))
* ctx claude-hook drives the agent status column ([1f12d17](https://github.com/Michael-JB/ctx/commit/1f12d175c4a0d278afe41207e7b968157bec222a))
* d archives a context instantly; delete confirms in the archived panel ([2a7d518](https://github.com/Michael-JB/ctx/commit/2a7d5189bba5a3b8096349c861d6bf8169e084f0))
* default repo for new contexts ([4ad08fe](https://github.com/Michael-JB/ctx/commit/4ad08fe646cc69196264f91d477c86ab80994ef7))
* fuzzy filter the focused panel by name with / ([50557b2](https://github.com/Michael-JB/ctx/commit/50557b263ebcbba13fa6ae184afce41f1a798d77))
* github column collapses the branch's PR into one cell ([361b1fc](https://github.com/Michael-JB/ctx/commit/361b1fcd476804e9d320f7aff6ab01185609dbe4))
* o opens the context's PR in the browser ([a42711d](https://github.com/Michael-JB/ctx/commit/a42711d0ed1aee40f478a46ef6fb52e81b1b0f06))
* pin the attached context on top and start the cursor below it ([02c6e4b](https://github.com/Michael-JB/ctx/commit/02c6e4b48edf5d7c9b15c0326feaf7f30adbaaba))
* theme colours configurable via a [theme] table ([81f75ca](https://github.com/Michael-JB/ctx/commit/81f75caf6b588ee592d69710a403f8f1a877cdef))


### Bug Fixes

* late status results no longer crash the closing TUI ([33b9e02](https://github.com/Michael-JB/ctx/commit/33b9e023a22b2735f67b5f1de864ea4eae188f00))
* lazygit-style dark cursor row keeps status colours readable ([4f25e04](https://github.com/Michael-JB/ctx/commit/4f25e045dc3ac27b6caa606f33afee54f71025a5))
* removing the current context no longer kills your client ([e26d3ec](https://github.com/Michael-JB/ctx/commit/e26d3ec724572347da478454faf059e278ff828b))
* slim gray scrollbars instead of the wide blue default ([e62d564](https://github.com/Michael-JB/ctx/commit/e62d56413c1068bdacc2a72464517cea8ae35631))
* unreadable blue table headers under ansi rendering ([1b631df](https://github.com/Michael-JB/ctx/commit/1b631df0bbdfad24a8d2519fb5f165a35b5409a6))


### Documentation

* agent-guided setup checklist and refreshed usage guide ([883a869](https://github.com/Michael-JB/ctx/commit/883a86984c9d933bdd796f1d69fd1253f19d4247))

## [0.5.0](https://github.com/Michael-JB/ctx/compare/v0.4.0...v0.5.0) (2026-08-08)


### Features

* keep context names unique whether archived or not ([36ae559](https://github.com/Michael-JB/ctx/commit/36ae559214a8e8c9747e3704262d748ced39f9ff))
* navigate with the arrow keys as well as h/j/k/l ([989fb9a](https://github.com/Michael-JB/ctx/commit/989fb9afea992ca56239094c68cdb89b8f80fb6e))


### Bug Fixes

* quit immediately even while a fetch or clone is running ([ae65476](https://github.com/Michael-JB/ctx/commit/ae65476b8386cfb13ed173b8b88e09f28cd0a85e))
* show the error popup instead of crashing when a git command fails ([af01e6d](https://github.com/Michael-JB/ctx/commit/af01e6d5cae83259050c6188337119f8b1cd1eeb))
* stop new contexts hanging and freezing the TUI ([5d717d0](https://github.com/Michael-JB/ctx/commit/5d717d05a3d07a08cf92bde772af9e89dfbb903a))
* stop slow status lookups delaying other status columns ([d6be30d](https://github.com/Michael-JB/ctx/commit/d6be30d1be17da3e0533f7f5e5710f2cbb3e4794))

## [0.4.0](https://github.com/Michael-JB/ctx/compare/v0.3.1...v0.4.0) (2026-08-08)


### Features

* compact the STATUS column to * and ↑n ([8c43c44](https://github.com/Michael-JB/ctx/commit/8c43c44216e7cb2f059c9fcbd14a2cf578fa2428))
* show agent, PR, and CI state via configurable status columns ([bc1e0bd](https://github.com/Michael-JB/ctx/commit/bc1e0bd714e3675509006b1bb03494c04cedd0da))
* **tui:** give the contexts table the full width ([0100b04](https://github.com/Michael-JB/ctx/commit/0100b04b436a193f94ddf58b4f8ce8ac82abe5a7))


### Bug Fixes

* **zellij:** drop the ZELLIJ_SOCKET_DIR override that broke switching ([d740e8e](https://github.com/Michael-JB/ctx/commit/d740e8ed484da4f3079759b10801c0dcd25098af))
* **zellij:** shorten session names past the macOS socket path limit ([ed81ec3](https://github.com/Michael-JB/ctx/commit/ed81ec361e6faee17b1a7ef0975b72845a4b62c7))
* **zellij:** surface switch failures in the TUI instead of dying silently ([cff6592](https://github.com/Michael-JB/ctx/commit/cff6592c97fe367b0bd52ede3b15ae241d2de220))

## [0.3.1](https://github.com/Michael-JB/ctx/compare/v0.3.0...v0.3.1) (2026-08-06)


### Bug Fixes

* support spaces in context names ([ccfc4ad](https://github.com/Michael-JB/ctx/commit/ccfc4adbcb46c822f716ab3a2f8edcc4c7669103))

## [0.3.0](https://github.com/Michael-JB/ctx/compare/v0.2.0...v0.3.0) (2026-08-05)


### Features

* archive contexts ([e23ace3](https://github.com/Michael-JB/ctx/commit/e23ace3a1cf0bdf8e18f2cd99b70e5b53d3ba09e))
* **tui:** panel-contextual keybindings help ([5522b5c](https://github.com/Michael-JB/ctx/commit/5522b5ceb415a77e88324e667fe70c905151bfe0))


### Bug Fixes

* **tui:** wrap long dialog messages ([046eb7b](https://github.com/Michael-JB/ctx/commit/046eb7b6bbcd08f396cd97511c4c3d57f3e11900))


### Documentation

* describe archiving in the README ([0801bc6](https://github.com/Michael-JB/ctx/commit/0801bc60e315edf217dd75b3bfadfb80cdfaeb3c))

## [0.2.0](https://github.com/Michael-JB/ctx/compare/v0.1.1...v0.2.0) (2026-08-04)


### Features

* sort contexts by recency ([a7bf82a](https://github.com/Michael-JB/ctx/commit/a7bf82a67553864f1b3378860165c891e808adf6))

## [0.1.1](https://github.com/Michael-JB/ctx/compare/v0.1.0...v0.1.1) (2026-08-04)


### Bug Fixes

* support repos without commits ([d214519](https://github.com/Michael-JB/ctx/commit/d2145194f893d9114911ed1d2de52ddcda51e8d0))

## 0.1.0 (2026-08-02)

Initial release.
