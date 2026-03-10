# djvu

Pure Rust library for parsing and rendering [DjVu](https://en.wikipedia.org/wiki/DjVu) documents.

## Disclaimer

This is an AI-assisted project. The implementation is semi-clean room: format
specs were derived from the public DjVu3 specification, but DjVu has many quirks
and underspecified details that the spec alone doesn't cover. In order to produce
readable rendering output, algorithm details were studied from
[djvu.js](https://github.com/RussCoder/djvujs) (GPL JavaScript renderer) and
[DjVuLibre](http://djvu.sourceforge.net/) (GPL C++ reference implementation).
No code was copied from either project. Test DjVu files are borrowed from
djvu.js, and golden test outputs are generated using DjVuLibre command-line
tools (`ddjvu`, `djvused`, `djvudump`).

For all intents and purposes this should be considered AI slop. That said, some
effort has been put into making the library performant and stable. It is used as
the DjVu rendering engine in [Bookokrat](https://bugzmanov.github.io/bookokrat/index.html).

## Features

- Full DjVu document parsing (single-page and bundled multi-page)
- IW44 wavelet decoding (background and foreground color layers)
- JB2 bilevel decoding (mask/text layer) with shared dictionary support
- 3-layer compositing with bilinear interpolation
- Text extraction (TXTz/TXTa OCR layers)
- Thumbnail extraction (TH44)
- Bookmark / table of contents (NAVM)
- Page rotation support
- Anti-aliased downscaling with configurable boldness
- No unsafe code in the public API crate
- No C dependencies

## Usage

```rust
let doc = djvu::Document::open("file.djvu")?;
println!("{} pages", doc.page_count());

let page = doc.page(0)?;
println!("{}x{} @ {} dpi", page.width(), page.height(), page.dpi());

// Render to RGBA pixmap
let pixmap = page.render()?;
let rgba_bytes: &[u8] = pixmap.as_ref();

// Or get raw RGB
let rgb = pixmap.to_rgb();

// Extract text (if OCR layer exists)
if let Some(text) = page.text()? {
    println!("{}", text);
}

// Thumbnails
if let Some(thumb) = page.thumbnail()? {
    println!("Thumbnail: {}x{}", thumb.width, thumb.height);
}
```

## Not Supported

- **Indirect multi-page documents** — only single-page and bundled multi-page files are supported; indirect documents (pages split across separate files) will return an error
- **Annotations and hyperlinks** (ANTz/ANTa) — not parsed
- **Encoding / writing** — this is a read-only library
- **CCITT/G4 fax encoding** — only JB2 and IW44 codecs are implemented

## Examples

```bash
cargo run --example render_page -- file.djvu 0 /tmp/page.ppm
cargo run --example extract_text -- file.djvu
cargo run --example page_info -- file.djvu
```

## License

GPL
