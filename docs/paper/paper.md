## Abstract

Strong chess engines score positions with neural networks, so a club player who asks one about a move gets an accurate number and no reason. Mate is a chess engine built to tutor that player, and it gives up strength to do it. Its evaluation is a JSON graph of five named blocks (material, loose pieces, king exposure, mobility and pawn structure), so every score breaks down into meters a club player can read. Fixed templates over those meters and board facts write its chat, with no language model. A search limited by depth and nodes reproduces bit for bit across Apple arm64 and x86\_64 Linux. A tutor grades each of the student's moves by the cost of that move against the engine's own best, at its own depth, and speaks only when something happened. A sparring mode plays down to the opponent in casual games.

We built Mate in seventeen increments, logged predictions before each one, and report the results together with our own re-analysis. Against a ladder of strength-limited Stockfish 19 opponents, Mate scores like a player of about 1870 ± 80 on Stockfish's own rating scale (`UCI_Elo`). The usual ±31 interval understates the uncertainty, because the logistic Elo curve fits poorly. Independent checks found no false claim in 922 statements about opponents' moves and 600 reasons in the quiet voice. The tutor gives 94% of moves the same grade at depth 5 as at depth 6. On Lichess, against 63 human opponents rated 685 to 2203, Mate won 129 of 136 games. Its accuracy by Stockfish 17.1 is about 90 in every rating band. Sparring visibly moved Mate's games toward level but did not change the results people got. We list the defects we found, including a repetition-detection error in the search and sparring comparisons made at mixed depths.

## 1. Introduction

A club player wants to know why a move was bad, and Mate is a chess engine built to tell them, even at the cost of winning fewer games. Its evaluation is a sum of five named terms that a club player already knows, and every sentence it says in the game chat comes from those terms or from facts on the board. No language model writes any of it.

Researchers who want an explanation from a chess engine usually start from a strong engine and work backwards. Stockfish and Leela Chess Zero, two of the strongest engines, evaluate positions with neural networks, so their numbers are accurate and opaque. Explanation research then either probes those networks for human concepts after training ([McGrath et al., 2022](https://pmc.ncbi.nlm.nih.gov/articles/PMC9704706)) or hands the engine's output to a language model and asks it to write commentary ([Kim et al., 2024](https://arxiv.org/pdf/2410.20811)). That second route gives a student fluent text that can be wrong about the board.

Mate takes the opposite trade. It gives up strength to keep three properties:

1. **Legibility.** The evaluation is a graph of blocks (material, loose pieces, king exposure, mobility, pawn structure), each with typed arguments and per-side meters. A score decomposes exactly into those meters.
2. **Checkable speech.** Every claim in a chat line (a piece is loose, a capture, a check, a block moved by its threshold) is a fact about a position, and an independent checker built on python-chess tests each claim against the board.
3. **Reproducibility.** A search limited by depth and nodes gives the same move, score, node count and sentence on an Apple arm64 laptop and an x86\_64 Linux server. A SHA-256 fingerprint over 602 reference positions is the test.

On top of that core sit the parts a student meets: a tutor that grades the student's moves, a quiet voice that speaks only when something happened, and a sparring mode that plays down to the opponent in casual games. Mate has played on Lichess as the bot account [Symbia](https://lichess.org/@/Symbia) since October 20, 2026, and in the browser through a WebAssembly build of the same engine.

In this paper we describe how Mate works, how we measured it, and where the measurements are weak. Section 6 includes our own re-analysis of the ratings and of 136 Lichess games with an outside engine, and Section 8 lists the defects that analysis found.

## 2. Related work

Chess engines differ in how they evaluate a position, how they search, and what they can tell a person about either. Mate sits at the far legible end of that range and the far weak end of strength.

| Engine or method | Evaluation | Search | Strength | What it can explain |
| --- | --- | --- | --- | --- |
| [Stockfish](https://www.chessprogramming.org/Stockfish_NNUE) (12 and later) | NNUE network, invented by Yu Nasu for shogi in 2018, in Stockfish since 2020 | Alpha-beta with heavy pruning, transposition table, tens of millions of positions a second | Top of every rating list | A score and a principal variation |
| [AlphaZero](https://ar5iv.labs.arxiv.org/html/1712.01815), Leela Chess Zero | Policy and value network learned by self-play | Monte Carlo tree search, about 80,000 positions a second for AlphaZero against Stockfish's 70 million | Superhuman | A score, visit counts; concepts recoverable by [linear probes](https://pmc.ncbi.nlm.nih.gov/articles/PMC9704706) after training |
| [Maia](https://arxiv.org/pdf/2006.01855) | Leela-style network trained on human games of one rating band | Single forward pass, no search | Plays like the band it was trained on; about 51 to 53% move-matching | Which move a human of that rating would play |
| Stockfish `UCI_LimitStrength` | Full-strength search | Picks a weaker move from its candidates by a statistical rule | Covers CCRL Blitz 1320 to 3190, fitted against the Stash engine ([search.h](https://raw.githubusercontent.com/official-stockfish/Stockfish/master/src/search.h)) | Nothing about why the weaker move was chosen |
| LLM commentary ([Jhamtani et al., 2018](https://aclanthology.org/P18-1154/); [Kim et al., 2024](https://arxiv.org/pdf/2410.20811)) | An engine or a learned model supplies concepts | None of its own | Not a player | Fluent prose; Kim et al. report that language models hallucinate chess content without engine concepts |
| **Mate** | Sum of five hand-written blocks with typed arguments and meters | Iterative-deepening alpha-beta with quiescence; no transposition table, no pruning beyond alpha-beta | About 1870 on Stockfish's `UCI_Elo` scale at 10 s + 0.1 s (Section 6.1) | Every term of the score, and template sentences whose claims are checked against the board |

Two lines of work are closest. Maia shows that a model can play like a person of a given rating, which Mate does not attempt: its sparring mode keeps the game close by choosing among its own candidate moves, not by imitating a human. Concept-guided commentary shows that language models write better commentary when an engine names the concepts first. Mate goes one step further back and makes the concepts the evaluation itself, then writes from fixed templates so that no sentence can say something the board does not support.

The cost is plain in the table. A hand-written five-term evaluation and an unpruned search are decades behind the state of the art in strength. The design bet is that a student rated under 2000 gains more from a partner whose reasons are true and visible than from one that is stronger and silent.

## 3. Architecture

Mate is about 9,700 lines of Rust with no dependencies. It borrows its structure from Ruse, Symbia Labs' audio-plugin framework: a board that behaves like a stateful actor, an evaluation described as a graph of blocks, and gates that check every build.

[[figure:architecture|Mate architecture: board, search, evaluation graph and the three consumers of a search report]]

The search asks the graph for a score at each leaf; the graph's five blocks report their meters alongside it, and the coach and the sparring chooser read the same report the bot plays from.

### 3.1 The board

The board holds bitboards by color and piece, the side to move, castling rights, the en passant square, the clocks and an undo stack. `make(move)` and `unmake()` are its only two messages, so nothing is copied going up and down the search tree. Move generation is pseudo-legal; each move is made, the mover's king is tested for attack, and the move is unmade. Pins, en passant discoveries and castling through check all fall out of that one test. The generator matches the published perft counts for the six standard test positions, including 119,060,324 nodes at depth 6 from the starting position.

### 3.2 Evaluation as a graph of blocks

The evaluation is a JSON graph, the same format as a Ruse plugin with positions in place of audio samples and the two sides in place of two channels. The default graph runs five blocks in parallel and sums their terms:

| Block | What it charges or rewards | Meters |
| --- | --- | --- |
| material | Pieces at 100, 320, 330, 500 and 900 centipawns | material per side |
| loose | A knight, bishop, rook or queen that is attacked and undefended, or attacked by a cheaper piece; half its value, a quarter for the side to move, which can still save it | count and centipawns per side |
| king | Missing shield pawns, open files beside the king, enemy pieces bearing on the squares around it, and a king shut on its back rank while the other side has a rook or queen | exposure per side |
| mobility | Squares reached by knights, bishops, rooks and queens | mobility per side |
| pawns | Doubled and isolated pawns charged, passed pawns rewarded by rank | pawns per side |

The frame carries one score per side; the position's value from White's side is White's channel minus Black's. Each block exposes typed arguments, and a branch's gain can be a number or an expression over named parameters, the ply, the move number, the game phase and the side to move. A personality is a different graph. The browser build ships two: Clean (the default) and Careless, which scales its own loose-piece term by `care + 0.2 * dice`, where `dice` is a seeded random modulator. Because the graph is data, a coach can turn the `sharp` or `safety` gain down and the engine plays with that blind spot.

### 3.3 Search

The search is textbook and deliberately so: iterative deepening, negamax alpha-beta, a quiescence search over captures and promotions, move ordering by the previous principal variation, most-valuable-victim captures and two killer moves per ply, and a one-ply extension for checks. Draws are scored for repetition, the fifty-move rule and bare kings; mate scores count distance. There is no transposition table, null move, late-move reduction or futility pruning. Each of those would make the search stronger and its node counts and scores harder to reproduce and explain. On an Apple-silicon Mac the release build searches about 1.7 million nodes a second; the WebAssembly build is 229 KB.

### 3.4 Determinism

The default graph computes with `f32` addition, multiplication, `min` and `max` only. IEEE 754 rounds those correctly on every platform, and Rust neither fuses nor reorders floating-point operations by default, so the same graph gives bit-identical scores on arm64 and x86\_64. The fingerprint tool searches 602 positions from real games at a fixed depth and hashes the move, depth, score, node count, principal variation and sentence of each. The Mac build (arm64, Rust 1.91) and the static Linux build (x86\_64 musl, Rust 1.97) give the same hash over 106,989,330 nodes; changing one parameter (`sharp=0`) changes 599 of the 602 rows, so the hash does notice a change.

## 4. The tutor

A student playing Mate gets a tutor that runs beside the engine, in a separate process mode (`uci coach`) on the bot and in a Web Worker in the browser. It never touches the search that plays the move. It takes two events per exchange: `student`, sent the moment the opponent's move arrives, and `tutor`, sent after Mate has moved. Each event carries a copy of the position, so the three lanes that read it share no state.

| Lane | Fed by | Says |
| --- | --- | --- |
| Game | `student` | What the student's move did: a capture, check, castle or promotion, and any piece of either side it left loose or attacked |
| You | `student` | A grade for the student's move, with a reason and the move Mate preferred |
| Me | `tutor` | What Mate's own move did and why, and who stands better when that changes |

One writer thread prints the lines, so no line is cut in two. A line more than one ply behind the newest event is dropped, and a takeback voids the lines still waiting for the plies it removes. In a fast game chat that trails the board is worse than chat that skips.

### 4.1 The judge

The You lane grades a move by what it cost at the engine's own depth. While the student thinks, Mate searches the position he must answer (the ponder). When he moves, the grade is one more search of the position after his move, one ply shallower:

```latex
\text{cost}(m) = \operatorname{clamp}\big(V_d(P)\big) - \operatorname{clamp}\big(-V_{d-1}(P \cdot m)\big)
```

```latex
\operatorname{clamp}(x) = \max(-1000, \min(1000, x))
```

Because the search is plain alpha-beta with no transposition table, the root value at depth d is exactly the best of the child values at depth d − 1, so the cost is never negative and is zero for the move the engine would play. The clamp at ±1,000 centipawns means that winning a little less in a won position costs nothing. A ponder cut short is discarded, never used, and both searches stop on depth and nodes (depth 6, 500,000 nodes by default) rather than on the clock. The grade is therefore a pure function of the position and the move: the same no matter how long the student thought or how busy the machine was.

| Cost (centipawns) | Class | Said aloud in the quiet voice |
| --- | --- | --- |
| 0 to 10 | Best | No |
| 11 to 49 | Good | No |
| 50 to 119 | Inaccuracy | Yes |
| 120 to 299 | Mistake | Yes |
| 300 or more | Blunder | Yes |

A move that allows a mate names the mate; a move that misses one says so. When there is a cause the engine can prove, the line gives it: the capture Mate can now make, a check or new attack its best reply makes, or the block that the student's move made worse than the best move would have ("It leaves your king more exposed," "It cramps your pieces," "It weakens your pawns").

### 4.2 Speaking only when something happened

The first version narrated every move. After playing it, the person it was built for said it "still narrates every move and doesn't say why." The quiet voice now has three rules:

1. **Always said, on the move it happens:** a forced mate for either side, a capture of a loose piece, a promotion.
2. **Said with a reason or not at all:** an attack on a loose piece, a piece rescued or defended, castling, a pawn to the center, a rook to an open file, a change of who stands better. A reason is a board fact, or a block that moved past its threshold: mobility up 20 centipawns, king exposure down 25, pawn structure up 20.
3. **A cooldown:** an ordinary line waits until Mate has made three moves since its last line. A line that waits is not kept for later.

Who stands better is said in words, not numbers, from Mate's side: about equal under 30 centipawns, then slightly better, better, clearly better and winning at 30, 80, 180 and 350. A word changes only when the score passes the next edge by 10 centipawns, so a score sitting on an edge does not flip the word back and forth.

### 4.3 Sparring

In casual games against people, Mate plays near the opponent's level. The search runs as usual. Only when Mate is clearly ahead does a chooser look at other moves: it screens every legal move with a depth-2 search, re-searches the best five against the best move, drops any that give back more than a per-move limit, and plays the one that leaves it closest to a seeded target a little above level, never below level.

| Opponent rating | Most Mate gives back in one move (centipawns) |
| --- | --- |
| under 1000 | 250 |
| 1000 to 1199 | 200 |
| 1200 to 1399 | 160 |
| 1400 to 1599 | 120 |
| 1600 to 1799 | 80 |
| 1800 to 1999 | 50 |
| 2000 and over, rated games, bots | 0 (full strength) |

"Level" is measured against Stockfish rather than against Mate's own number, which reads optimistic near equality (Section 6.4). The chooser is off for the first ten half-moves and when Mate has under 20 seconds on its clock, and the grade in the You lane still comes from the full-strength judge, so "I would play X" names the best move, not the move Mate chose.

## 5. Method: predictions first

We built Mate in seventeen increments, B1 to B17, between October 18 and 27, 2026. Each increment opened with a written brief and a numbered list of predictions, which we logged on Symbia's ledger before any code or run that could score them. Each closed with a results table that scored every prediction as held, broke or open, with the number. A prediction rewritten after a run would no longer be a prediction, so we logged revisions as new records beside the originals, before the run that scored them.

The predictions are specific enough to fail. Some examples, with how they came out:

| Increment | Prediction | Outcome |
| --- | --- | --- |
| B3 | At depth 2 the engine takes the loose piece in at least 97% of 300 puzzles cut from real games | Broke: 69.3%. The misses were mostly the puzzle detector's blind spots, such as the mover's own queen being en prise |
| B4 | The fitted rating lies between 1500 and 2000 | Held: 1868 |
| B8 | The static Linux build gives the Mac's fingerprint over 602 positions | Held: identical hash, 106,989,330 nodes each |
| B10 | Rollback after a failed update leaves the installed tree byte-identical | Broke on the live server, though it held in all 129 sandbox cases |
| B13 | The class a move gets at depth 5 equals its class at depth 6 for at least 80% of moves | Held: 556 of 590 (94.2%) |
| B15 | Sparring at 1700 scores 50 to 85% against Stockfish at `UCI_Elo` 1700 | Broke: 48.8% |
| B16 | At least 60% of graded inaccuracies and worse give a cause | Broke: 28%, then 43% after one added clause |

Three kinds of check back the predictions:

- **Gates.** `cargo test --release` runs perft counts, mirror symmetry of every block, material known to the centipawn, mates in one and two, a speed floor, and the UCI round trip. The library test count grew from 0 to 98 across the increments.
- **An outside judge of the words.** `tools/narrate_check.py` and its successors re-derive every claim in every sentence (the piece and square named, the capture, the check, the loose piece, the size and sign of a block change) with python-chess, which shares no code with Mate. The checker was itself tested on deliberately false sentences and caught all six.
- **Fingerprints.** The SHA-256 over 602 reference positions is recomputed whenever a change is meant to leave the search alone. B11, B12, B14, B16 and B17 changed only what Mate says; the search columns of the fingerprint stayed byte-identical through all five.

One limit applies to the word checks throughout. The checker writes Mate's definition of a loose piece a second time in Python, so it proves the sentences follow the definition, not that the definition is the right one.

## 6. Results

### 6.1 Strength against a Stockfish ladder

Mate scores like a player of about 1870 on Stockfish's `UCI_Elo` scale at 10 s + 0.1 s, but no single rating describes it well. The ladder played 840 games against Stockfish 19 with `UCI_LimitStrength` at six levels, openings from a fixed set played once with each color, no illegal move, crash or time loss.

[[figure:ladder|matches/sweep and matches/refine, 840 games, October 19 to 20, 2026; refit by the author]]

The maximum-likelihood fit gives **1868 ± 31** (95%, with the two games of each opening treated as one observation). That interval assumes the logistic Elo curve is the right shape, and it is not: the rating each level implies on its own climbs from about 1760 at the bottom of the ladder to 2030 at the top (χ² ≈ 36 on 5 degrees of freedom). A curve with a free slope fits far better (likelihood-ratio statistic 28, one degree of freedom) and is 36% flatter than Elo's. Fitting the three lowest levels alone gives 1782, the three highest 1916. Widening the interval for that misfit gives about **1870 ± 80**, which is the figure to quote.

We have not tested why the curve is flatter. One plausible cause is the opponent rather than Mate: a strength-limited Stockfish weakens by choosing among its candidate moves, and that kind of error may not scale with level the way human error does. Stockfish documents its scale as CCRL Blitz, fitted against the Stash engine ([search.h](https://raw.githubusercontent.com/official-stockfish/Stockfish/master/src/search.h)), so the number is neither a FIDE nor a Lichess rating.

Time matters. Two hundred games at 60 s + 0.6 s against level 1900 scored 56.5% against 46.5% at the shorter control, a fitted 1945 ± 49. The difference of 77 points is 2.6 standard errors (p ≈ 0.009); compared directly at level 1900, with no model, it is z = 2.1 (p ≈ 0.03). The original write-up called the gap 1.3 standard errors by dividing by 95% half-widths instead of standard errors; the corrected figure says Mate gains real strength from a longer clock, as an engine with no transposition table should.

### 6.2 Are the words true?

No sentence Mate wrote in the reference sets made a false claim about the board. The test sets are real games, not positions picked to make the sentences look good: twelve of the student's own games (602 positions), and later all 39 of Mate's first Lichess games plus 97 held-out games downloaded after the rules were frozen.

| Check | Set | Result |
| --- | --- | --- |
| Claims about the opponent's move (capture, pieces, squares, castling side, promotion, check, every loose piece named) | 602 moves | 922 claims, 0 false |
| Most valuable newly loose piece of the mover is named | moves that left one | 49 of 49 |
| Hanging piece named in Mate's own sentence | 300 puzzle positions | 273 of 300 (91%); of the 27 misses, 16 were forced mates where the sentence names the mate instead |
| Reasons in the quiet voice (development, center, open file, block change with size and sign, material) | 39 games | 600 claims, 0 false after one checker fix and one code fix |
| Mates, loose-piece captures and promotions still spoken in the quiet voice | 39 + 97 games | 0 of 202 and 0 of 437 missing |
| Moves on which Mate speaks at all | 39 games | 68% in B16, 31.6% after the cooldown (B17); 28.2% on the held-out 97 |

The judge is stable across depth. Graded at depth 5 and depth 6, 556 of 590 student moves get the same class (94.2%); 32 of the 34 changes move to a neighboring class, 19 of them between Best and Good, which the quiet voice does not say aloud. With class frequencies of 47% Best, 20% Good, 13% Inaccuracy, 12% Mistake and 8% Blunder, agreement by chance would be about 30%, so Cohen's κ is roughly 0.92. Stability is not accuracy: the judge agrees with itself, and its errors are the errors of a depth-6 search over a five-term evaluation. It cannot tell opening moves apart; it grades 1.g4 as Good (cost 19) and 1.a3 as Good (cost 16).

The grade does not depend on timing. Judging the same 303 student moves with the ponder always cut short and with the ponder always allowed to finish gives byte-identical lines. On the Mac the grade arrives a median 0.03 s after the move with a finished ponder and 0.59 s cold; on the two-core server that hosts the bot, 1.33 s cold.

### 6.3 Sparring against Stockfish

Sparring lowers Mate's score against Stockfish, but less evenly than designed. Each run is 40 games at 60 s + 0.6 s against Stockfish 19 at the opponent's level, so each score has a 95% interval of about ±15 points.

| Run | vs 1320 | vs 1500 | vs 1700 |
| --- | --- | --- | --- |
| Sparring off: score, share lost | 97.5%, 2.5% | 82.5%, 17.5% | 88.8%, 10.0% |
| First chooser, no offset | 80.0%, 20.0% | 41.2%, 52.5% | 33.8%, 65.0% |
| Second chooser, offset 130 | 83.8%, 12.5% | 66.2%, 30.0% | 48.8%, 50.0% |

The first chooser aimed at a small lead by Mate's own count and walked into positions Stockfish judged lost, because Mate's evaluation reads optimistic near equality. The second subtracts a fixed offset of 130 centipawns before aiming. It is a level partner at 1320 and 1500 by Stockfish's own view of the games and a coin flip at 1700. With 40 games a level these runs separate the first chooser from the second, but not one band's target from its neighbor's.

### 6.4 How optimistic is Mate's own number?

Mate's evaluation is compressed near zero, not simply shifted. Over the sparring ladder games with sparring off and in the first chooser, from half-move 20 on, a position Mate scores 0 to 100 centipawns has a median Stockfish view of about −100 to −115 for Mate, 100 to 200 reads level, 200 to 300 reads +30 to +120, and 300 to 500 reads +350 to +480. A single offset of 130 is a fair local correction where the sparring band sits and wrong elsewhere.

The mapping also depends on how the move was chosen. In the sparring-on games, a reported score of 300 to 500 meets a median Stockfish view of +64 (371 positions), against +480 with sparring off. A sparring move's reported score is the full-depth score minus a shallow estimate of what the move gives back, and that hybrid is the most optimistic number Mate prints. Part of a constant optimism is also structural: the root of any search reports the maximum of many noisy estimates, which biases it upward. The offset should be re-measured per selection policy, not once.

### 6.5 Outside data: 136 games on Lichess

Against people on Lichess, Mate has won 129, drawn 3 and lost 4 of the 136 games we could export (October 20 to 27, 2026). Every opponent was human and every game came from a challenge: 28 rated, 108 casual, 69 rapid and 67 blitz, against 63 different players rated 685 to 2203 (median 1567). The account's profile showed 144 games when this paper was written; the remaining eight could not be retrieved because Lichess's per-user export was unreachable from our network, so we exported by game ID.

To judge the quality of play from outside, we re-analyzed every position of the 136 games with Stockfish 17.1 at 300,000 nodes a position, independent of anything Mate computes. Accuracy below uses Lichess's published per-move formula ([Lichess](https://lichess.org/page/accuracy)) averaged with a plain mean, which reads a few points higher than Lichess's own game figure; on the 15 games Lichess had analyzed itself, Mate's accuracy there had a median of 87.

| Opponent rating | Games | Mate won, drew, lost | Mate accuracy | Opponent accuracy | Mate median centipawn loss | Opponent median centipawn loss |
| --- | --- | --- | --- | --- | --- | --- |
| under 1000 | 24 | 24, 0, 0 | 90.7 | 86.5 | 27 | 58 |
| 1000 to 1399 | 24 | 24, 0, 0 | 90.1 | 84.9 | 28 | 69 |
| 1400 to 1799 | 40 | 36, 2, 2 | 90.6 | 87.3 | 32 | 55 |
| 1800 to 1999 | 18 | 18, 0, 0 | 89.6 | 86.2 | 33 | 52 |
| 2000 to 2299 | 30 | 27, 1, 2 | 89.2 | 87.0 | 35 | 49 |

Mate's own accuracy is flat across the bands, while its opponents' centipawn loss falls as their rating rises. Against the 2000-plus group the gap in accuracy is about two points, and that group accounts for one of Mate's three draws and two of its four losses. The bot's Lichess ratings, 3030 in blitz and 3008 in rapid, are provisional figures from 28 rated wins under Glicko-2 and say nothing reliable about strength.

**Sparring did not change the results people get.** B15 predicted that people would take at least 25% of the points in casual games once sparring was live (from 22:09 UTC on 23 October). In the 49 casual games since then against players under 2000, Mate scored 47 wins and 2 draws: the opponents took 2%. Sparring is visibly active in those games all the same. The share of Mate's moves after move 10 at which Stockfish rates the game between −0.5 and +2.0 for Mate rose from 20% before sparring to 38% after, and Mate's median centipawn loss rose from 30 to 37. Mate does give ground; its human opponents give back more. The chooser's per-move limit (120 centipawns against a 1500 player) is smaller than a typical club-level blunder, so a game that Mate holds near level is still decided by the opponent's next loose piece. The comparison is observational, before against after, and the opponents in the two periods are not the same people.

## 7. Case study: two games on the Sparring page

The Sparring page is the WebAssembly build of Mate in a single web page: the board, the five block meters for each side, the chat, and faders for search depth and the active personality's parameters. For this paper we drove it headless in Chromium and let Stockfish 17.1 at a low Skill Level play the student's side, picking each move in 0.05 s. The page carries a build from before the B17 cooldown, so Mate speaks on more of its moves here than the hosted bot does now. Both games are reproducible from the move lists below.

### 7.1 Clean personality, depth 4, against Skill Level 3

The student played White; Mate played Black with the default graph at the page's default depth of 4 plies. Mate lost material in the middlegame and, after 80 half-moves, Stockfish at depth 18 rated the position +7.25 for the student.

[[image:sparring-clean.jpg|The Sparring page after 16...fxe2: board, block meters and the chat that led to the queen trade|The Sparring page after 16...fxe2, with the board, the block meters and the chat that led to the queen trade.]]

The chat around the turning point reads, in order:

> You moved your pawn to d6. It attacks my queen on e7 and knight on c7, which are loose.
>
> I am attacking your queen on e2, which is loose. You are clearly better. I looked 2 moves ahead.
>
> You took my loose queen on e7 with your pawn. It attacks my bishop on f8, which is loose.
>
> You made a blunder that costs about 3 pawns. I can take your queen on e2. I would play rook takes pawn on f3.
>
> I took your loose queen on e2. Your rook on f1 is loose. You are slightly better because your pieces have more room.

Every claim in those lines is true of the board, and the grade points the right way. Stockfish's best move for White at that point was the one Mate recommended, Rxf3 (+5.55), against the move played, dxe7 (+4.10). Mate overstated the cost: 145 centipawns by Stockfish's count is a Mistake, not a 3-pawn Blunder. Mate's own move 15...f3 was its worst of the game; Stockfish preferred 15...Qxd6 by 1.4 pawns. At depth 4 a five-term evaluation does not see that its counter-attack loses a piece two moves later. That is the browser build at its default depth: true sentences, and a player a weak engine can beat.

Moves: 1. Nf3 e6 2. e4 b6 3. Nc3 Ba6 4. Bxa6 Nxa6 5. a3 g5 6. h3 h6 7. d4 c6 8. Ne5 Nf6 9. Qe2 Nc7 10. f4 gxf4 11. O-O c5 12. d5 exd5 13. Nxd5 Nfxd5 14. exd5 Qe7 15. d6 f3 16. dxe7 fxe2 17. exf8=Q+ Rxf8 18. Re1 f6 19. Nf3 Rh8 20. Rxe2+ Ne6 21. c4 O-O-O 22. Bd2 Ng5 23. Nh2 Rde8 24. Rxe8+ Rxe8 25. Nf1 Re2 26. Re1 Rxe1 27. Bxe1 Kb7 28. Kf2 Ne4+ 29. Ke2 Nd6 30. Ne3 Kc6 31. Kd1 a5 32. Ke2 Nf7 33. Bg3 d6 34. b3 Ne5 35. a4 h5 36. Nd5 f5 37. Bxe5 dxe5 38. Kd3 Kb7 39. Nf6 h4 40. Nh7 Ka6 (stopped).

### 7.2 Careless personality against Skill Level 5

The Careless graph charges Mate for its own loose pieces at only 0.3 of the normal rate, plus seeded noise. It is meant to be beaten by a student who notices loose pieces. Mate played White; the student played Black and mated it on move 31.

[[image:sparring-careless.jpg|The final position of the Careless game: Mate, as White, mated after leaving pieces loose|The final position of the Careless game. Mate, as White, was mated after leaving pieces loose.]]

The personality behaved as designed: Mate moved into attacks on its own pieces repeatedly ("It attacks my knight on b5, which is loose" three times in eight half-moves) while still grading the student's moves at full strength, because the judge always uses the default graph. Two defects show in the last lines. The Game lane describes the mating move as "It gives check" rather than as mate, and Mate's own lane says nothing about having been mated. Both are template gaps, not false claims, and both are on the list in Section 8.

Moves: 1. e3 c6 2. b3 g6 3. Bb2 f6 4. Ba3 e5 5. Bxf8 Kxf8 6. Nc3 Qe7 7. Qg4 f5 8. Qh3 Kg7 9. Nf3 Nf6 10. a3 d5 11. Qg3 f4 12. Qh4 e4 13. Nd4 h6 14. exf4 c5 15. Nde2 Nc6 16. Ng3 Rf8 17. Be2 Kh7 18. O-O-O c4 19. Kb2 b5 20. Nxb5 Rb8 21. bxc4 dxc4 22. a4 Qb4+ 23. Kc1 Qxa4 24. d3 Rxb5 25. dxc4 Ra5 26. c3 Qb3 27. Kd2 Rd8+ 28. Ke1 Ra1 29. Rxa1 Qxc3+ 30. Kf1 Qxa1+ 31. Bd1 Qxd1#.

## 8. Limitations and known defects

We found the defects below by re-reading the code against its own documentation and by testing a copy of the engine. None has been fixed at the time of writing.

| Area | Defect | Evidence | Effect |
| --- | --- | --- | --- |
| Search | Repetitions of positions from the game history are detected only at odd plies; `repeated()` steps through the history with a fixed parity | With history \[A, B, R\], `repeated(A)` returns false while `repeated(B)` returns true | Mate can misjudge lines that repeat an earlier game position, in either direction |
| Search | Check extension applies only above the horizon; a check given on the last ply drops into quiescence, which stands pat while in check | Read from `search.rs` | Mates delivered on the final ply are missed; mate-in-1 needs depth 2 |
| Sparring | Candidate moves are compared at whatever depth a 30,000-node search finishes, not at one depth as documented | 145 of 284 comparison searches stopped short at root depth 5; 39 of 70 positions compared candidates at mixed depths; with the cap lifted, 45 of 70 positions chose a different move | The give-back is partly noise from odd and even search depths |
| Sparring | The "never below level" floor subtracts a depth-4 loss from a full-depth score | Read from `spar.rs` | The floor is an estimate, not a guarantee |
| Evaluation | No piece placement or castling term; the king block charges a castled king on its back rank | B15 and B16 findings; Mate castled in 6 of its first 39 games, its opponents in 28 | Mate cannot say castling made it safer, and plays the opening poorly |
| Evaluation | Mate's score is compressed near zero and its bias depends on how the move was chosen | Section 6.4 | A fixed offset of 130 is a local correction only |
| Tutor | The judge has no game history: no repetition, no fifty-move count | B13 brief | Grades near a repetition can be wrong |
| Tutor | The mating move is described as a check, and Mate says nothing when mated | Section 7.2 | Template gap |
| Tutor | Most of what a bad move costs is in no block: 95 of 166 inaccuracies and worse name no cause | B16 results | The "why" is missing for the majority of grades |
| Measurement | The word checker re-implements Mate's loose-piece definition | B7 | It proves consistency with the definition, not the definition |

The ratings carry their own limits. The ladder rating is on Stockfish's scale and depends on which levels are played (Section 6.1). The hosted bot searches at about 0.7 million nodes a second, 40% of the speed of the laptop on which the ladder was run, so its strength on Lichess is lower than 1870 on that scale by an amount not yet measured. And every ladder result comes from 10 s and 60 s games against one opponent family.

## 9. Future work and conclusion

The next increments follow from the defects. A line of play in the judge, rather than its single best reply, would let a grade name its cause for more than the 43% of bad moves it covers now. Placement, castling and king-in-the-center blocks would fix the opening and let Mate say why castling helped. Fixing the repetition parity, comparing sparring candidates at one completed depth, and measuring the evaluation offset per selection policy are each small changes with a test already written in this paper's analysis. Strength at the hosted machine's speed needs its own ladder.

The project's larger claim is a method. With Mate we show that an engine can be built so that every number it uses decomposes into named terms, code that shares nothing with the engine checks every sentence it says against the board, and a prediction that could fail comes before every change. It is a modest player, about 1870 on Stockfish's scale with real uncertainty around that, and it lost to a weak Stockfish setting in our browser test at its default depth. In return a student gets an opponent and tutor that does not invent reasons, plays near their level in casual games, and says the same thing every time it sees the same position.

## Disclosure

We built Mate and wrote this paper with Claude, an AI system made by Anthropic, working under Brian Gilmore's direction. Claude wrote the code in seventeen increments and logged each increment's predictions on Symbia's ledger before the runs that scored them (Section 5). Claude also ran the re-analysis in Section 6 and wrote the text.

Data for this paper: the Mate repository's increment briefs B1 to B17 and `matches/` archives; 136 Lichess games of the Symbia account exported on October 27, 2026; two browser games played for Section 7; Stockfish 17.1 analysis by the author.

## References

1. Silver, D., Hubert, T., Schrittwieser, J., et al. [Mastering Chess and Shogi by Self-Play with a General Reinforcement Learning Algorithm](https://ar5iv.labs.arxiv.org/html/1712.01815). arXiv:1712.01815, 2017.
2. McIlroy-Young, R., Sen, S., Kleinberg, J., Anderson, A. [Aligning Superhuman AI with Human Behavior: Chess as a Model System](https://arxiv.org/pdf/2006.01855). KDD 2020.
3. McGrath, T., Kapishnikov, A., Tomašev, N., Pearce, A., et al. [Acquisition of chess knowledge in AlphaZero](https://pmc.ncbi.nlm.nih.gov/articles/PMC9704706). PNAS 119(47), 2022.
4. Jhamtani, H., Gangal, V., Hovy, E., Neubig, G., Berg-Kirkpatrick, T. [Learning to Generate Move-by-Move Commentary for Chess Games from Large-Scale Social Forum Data](https://aclanthology.org/P18-1154/). ACL 2018.
5. Kim, J., Goh, J., Hwang, I., Cho, J., Ok, J. [Bridging the Gap between Expert and Language Models: Concept-guided Chess Commentary Generation and Evaluation](https://arxiv.org/pdf/2410.20811). arXiv:2410.20811, 2024.
6. Chessprogramming wiki. [Stockfish NNUE](https://www.chessprogramming.org/Stockfish_NNUE).
7. Stockfish developers. [src/search.h](https://raw.githubusercontent.com/official-stockfish/Stockfish/master/src/search.h): Skill and `UCI_Elo` range and calibration notes.
8. Lichess. [Accuracy](https://lichess.org/page/accuracy): win-percentage and move-accuracy formulas.
9. Lichess. [Symbia (BOT)](https://lichess.org/@/Symbia): the account Mate plays as.
