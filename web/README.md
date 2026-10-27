# akler on the web

The same Rust program, compiled to WebAssembly (`wasm32-wasip1-threads`) and run
in the browser by the [Wasmer SDK](https://www.npmjs.com/package/@wasmer/sdk),
with [xterm.js](https://xtermjs.org/) as the terminal.

## Build and run locally

```sh
rustup target add wasm32-wasip1-threads   # once
cd web
npm install                               # once
npm run build                             # writes web/dist
npm run serve                             # http://localhost:8080/
npm run check                             # native and wasm give the same output
```

`npm run check` runs every served layout's evaluation and two short
fixed-seed benches natively and under the Wasmer CLI (`~/.wasmer/bin/wasmer`,
or `$WASMER`), and lists any output that differs; timings are ignored.

`npm run build` compiles `akler.wasm`, copies the page and its runtime into
`web/dist`, and bundles the data: layouts, the reddit corpus (`CORPORA` in
`build.mjs`), the scissor test's word list (`akler scissor-words`) and
`akler.conf` with `workers = 0`, so the optimizer uses one worker per core.

## Hosting

`web/dist` is a static site. Threads need a cross-origin-isolated page, so the
host must send

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

The build writes them to `dist/_headers`, which Netlify and Cloudflare Pages
read. Elsewhere, such as on GitHub Pages, `coi-serviceworker.js` adds them in the
browser; the first visit reloads the page once to install it.

GitHub Pages: `npm run build && npm run deploy` force-pushes `web/dist` to the
`gh-pages` branch. Then, once, set the repository's Settings → Pages source to
that branch; the site appears at `https://<user>.github.io/<repo>/`.

## In the browser

- Everything runs on the visitor's machine; nothing is uploaded.
- Changed settings, layouts, scissor results and optimizer output are saved in
  the browser (IndexedDB) and restored on the next visit. **Files** downloads
  them, **Upload layouts** adds layout files, **Reset** forgets them.
- The font menu in the top bar switches between Gohu (default), Monocraft,
  Serious Shanns and Comic Mono; the choice is remembered. Gohu is a bitmap
  font, converted from the gohufont release's `gohufont-uni-14.bdf` and
  `gohufont-uni-14b.bdf` by `python3 bdf2font.py` (needs fontTools and
  brotli). Monocraft's box drawing comes from `src/fonts/MonocraftBox.woff2`
  (`python3 boxfont.py`).
- The sun/moon button switches between Dark (black, the default) and Light
  (a slightly lighter grey), both with white text. Ctrl+Up / Ctrl+Down
  zoom the terminal; Ctrl+L and Ctrl+R go to the browser, not to akler.
- `?args=...` runs a command instead of the menu, for example
  `?args=bench layouts/night.jsonc corpus-reddit --seeds 2`.
- The home menu has no quit; `q`, Esc and Ctrl-C only go back.
- Not available: building corpora from raw text unless you upload it.
