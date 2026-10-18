# chess

A chess engine built the way RUSE builds a plugin: the board is a stateful actor, evaluation is a graph of blocks with typed arguments and meters, and gates check every build. It exists to tutor, so every verdict decomposes into the blocks that produced it.

Increments are briefs under `docs/`: B1 is the board and move generation.

```
cargo test --release                 # the gates
cargo test --release -- --ignored    # the deep perft counts
cargo run --release --bin perft -- 5 # nodes per second from the start position
```
