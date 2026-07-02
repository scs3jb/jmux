# Claude state sprites

The animated octopus GIFs shown bottom-right of sidebar workspace rows when a
Claude agent is active (`src/ui/state_sprite.rs`):

- `working.gif` — octopus hammering an anvil (main turn running)
- `needs_input.gif` — octopus with a sparkler (question/menu on screen)
- `waiting.gif` — octopus typing at a laptop (background task running)

They are the same sprites `~/src/deck` shows on the e-ink deck, pre-processed
to a transparent background with deck's own keying pipeline
(`deck/render.py:_load_sprite` — flood-fill background keying, ground-line
scrub, laptop highlight, union-bbox crop), preserving the original frame
timings. To regenerate after deck's sprites change, run a script that loads
each sprite via `deck.render._load_sprite` and re-saves it as a GIF with
binary transparency (palette index 255, `disposal=2`) into this directory.
