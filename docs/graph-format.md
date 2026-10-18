# Describing an evaluation as a graph

A graph is a JSON object: a node, with optional `params` and `modulators`. The same format as a RUSE plugin, with positions in place of samples and the two sides in place of the two channels.

## Nodes

- `{"block": "loose", "args": {"undefended": "0.5 * sharp"}}` runs one block. Arguments left out use their defaults.
- `{"chain": [node, node, ...]}` runs its nodes one after another; each adds its term.
- `{"parallel": [{"node": node, "gain": 0.5}, ...]}` gives every branch the same input and adds what each contributes, scaled by its gain. `gain` is optional and may be an expression.

Any node can also have:
- `"bypass"`: a parameter name or expression. At 0.5 or more the node is skipped.
- `"mix"`: 0 to 1, a number or expression. 0 leaves the node's contribution out, 1 keeps all of it.
- `"only"`: a list of sides the node applies to, `0` for White and `1` for Black. A weakness block with `"only": [1]` makes a bot playing White blind to its own loose pieces while it still punishes Black's.

## Signal

The frame holds one score per side in centipawns, positive when good for that side. A block adds its term to each side. The score of the position from White's side is White's channel minus Black's.

## Parameters

`"params": [{"id": "sharp", "min": 0, "max": 1, "default": 1, "doc": "..."}]` declares the knobs an expression can read. `render --set id=value` sets one; a UCI build exposes each as an option.

## Expressions

Names: any parameter id, any modulator id, `ply` (half-moves played in the game), `move` (the move number), `phase` (0 at the start, 1 with only kings and pawns left), `stm` (0 when White is to move, 1 when Black is) and `pi`.
Operators: `+ - * / ^` and the comparisons `< > <= >=`, which give 1 or 0. Division by zero gives 0.
Functions: `min(a,b) max(a,b) clamp(x,lo,hi) abs(x) pow(a,b) exp(x) ln(x) log2(x) sqrt(x) sin(x) cos(x) tanh(x) floor(x) round(x) mix(a,b,t) if(c,a,b)`.
Expressions are read once per position. `"gain": "if(move < 15, 0.2, 1)"` is a term that barely counts in the opening.

## Modulators

`"modulators": [{"id": "dice", "shape": "random", "rate": 1, "seed": 7}]` adds a signal any expression can read by its id, from -1 to 1, advancing with the ply. Shapes: `sine`, `triangle`, `saw`, `square`, `random` (sample and hold, repeatable from its seed). `rate` is cycles per ply and may be an expression over the parameters.

## Meters

Every block reports meters, listed by `render` as `block.meter`. A repeated block is numbered: `loose`, `loose2`.

## Latency

A block that looks ahead reports plies of latency; the graph sums them. No block looks ahead yet.

## Blocks

### material
Counts each side's pieces at the values given. The classic scale is 100, 320, 330, 500, 900.
- pawn: 0 to 300 cp, default 100.
- knight: 0 to 1000 cp, default 320.
- bishop: 0 to 1000 cp, default 330.
- rook: 0 to 1500 cp, default 500.
- queen: 0 to 3000 cp, default 900.
- meters: white, black.

### loose
Charges each side for pieces that can be won: attacked and undefended, or attacked by something cheaper. The side to move is charged a fraction, since it can still save the piece.
- undefended: 0 to 1, default 0.5. Fraction of an undefended piece's value charged.
- cheaper: 0 to 1, default 0.5. Fraction of the difference charged when a cheaper piece attacks a defended one.
- own_turn: 0 to 1, default 0.25. Scale for the side to move.
- meters: white_loose, black_loose (counts), white_cp, black_cp.

### king
Charges each side for an exposed king: pawns missing from the shield in front of it, open files beside it, enemy pieces bearing on the squares around it, and a king shut in on its back rank while the other side still has a rook or queen.
- shield: 0 to 60 cp, default 15. Per pawn missing from the three squares in front of the king.
- open: 0 to 60 cp, default 10. Per file beside or under the king with no own pawn.
- attack: 0 to 60 cp, default 10. Per enemy piece attacking a square next to the king.
- luft: 0 to 150 cp, default 30. A king on its back rank with no empty square in front of it, while the other side has a rook or queen.
- meters: white_exposure, black_exposure.

### mobility
Rewards each side for the squares its knights, bishops, rooks and queens reach, not counting squares its own pieces stand on.
- knight 4, bishop 5, rook 2, queen 1 cp per square, each 0 to 20.
- meters: white, black.

### pawns
Charges doubled and isolated pawns and rewards passed pawns, more for each rank they have advanced.
- doubled: 0 to 60 cp, default 10. isolated: 0 to 60 cp, default 15. passed: 0 to 100 cp per rank, default 20.
- meters: white, black.
