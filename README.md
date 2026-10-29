# Mate

A chess engine built the way Ruse builds a plugin: the board is a stateful actor, evaluation is a graph of blocks with typed arguments and meters, and gates check every build. It exists to tutor, so every verdict decomposes into the blocks that produced it.

Increments are briefs under `docs/`, B1 through B18. B1 is the board and move generation, B2 the block model and the graph, B3 the search, UCI and the browser build. B4 to B11 add narration, the Lichess bot and the deploy scripts. B12 to B18 add the coaching lanes, the judge, sparring at the opponent's level, the tutor voice and two search fixes.

```
cargo test --release                              # the gates
cargo test --release -- --ignored                 # the deep perft counts
cargo run --release --bin perft -- 5              # nodes per second from the start position
cargo run --release --bin render -- default g.pgn # a game through the default graph, meters per ply
cargo run --release --bin uci                     # the engine for a GUI or a bot
tools/wasm.sh                                     # the engine for a browser
cargo test --release --test puzzle_study -- --ignored --nocapture  # how the engine treats the puzzles, by depth
```

Personalities are graphs under `graphs/`; the format is `docs/graph-format.md`.

## License

MIT. See `LICENSE`.
