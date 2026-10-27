"""Convert a BDF bitmap font into a WOFF2 outline font made of square pixels,
so browsers can use it. At a CSS font size equal to the BDF's pixel size, each
bitmap pixel covers one CSS pixel.

    python3 bdf2font.py FONT.bdf OUT.woff2 FAMILY STYLE
"""
import re
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables import ttProgram

PIXEL = 100  # font units per bitmap pixel


def parse(path):
    text = open(path, encoding="latin-1").read()
    header = text.split("STARTCHAR", 1)[0]
    number = lambda name: int(re.search(rf"^{name} (-?\d+)", header, re.M).group(1))
    glyphs = []
    for block in re.findall(r"STARTCHAR.*?ENDCHAR", text, re.S):
        code = int(re.search(r"^ENCODING (-?\d+)", block, re.M).group(1))
        if code < 0:
            continue
        width = int(re.search(r"^DWIDTH (\d+)", block, re.M).group(1))
        w, h, x, y = map(int, re.search(r"^BBX (\S+) (\S+) (\S+) (\S+)", block, re.M).groups())
        rows = block.split("BITMAP", 1)[1].split()[:-1]
        bits = [[(int(row, 16) >> (len(row) * 4 - 1 - col)) & 1 for col in range(w)] for row in rows]
        glyphs.append((code, width, x, y, h, bits))
    return number("FONT_ASCENT"), number("FONT_DESCENT"), glyphs


def outline(x, y, h, bits):
    """One rectangle per horizontal run of set pixels, wound clockwise."""
    pen = TTGlyphPen(None)
    for row, line in enumerate(bits):
        top = (y + h - row) * PIXEL
        col = 0
        while col < len(line):
            if not line[col]:
                col += 1
                continue
            start = col
            while col < len(line) and line[col]:
                col += 1
            left, right = (x + start) * PIXEL, (x + col) * PIXEL
            pen.moveTo((left, top - PIXEL))
            pen.lineTo((left, top))
            pen.lineTo((right, top))
            pen.lineTo((right, top - PIXEL))
            pen.closePath()
    return pen.glyph()


def main(source, target, family, style):
    ascent, descent, glyphs = parse(source)
    empty = TTGlyphPen(None).glyph()
    names = [".notdef"] + [f"uni{code:04X}" for code, *_ in glyphs]
    outlines = {".notdef": empty}
    metrics = {".notdef": glyphs[0][1] * PIXEL}
    cmap = {}
    for (code, width, x, y, h, bits), name in zip(glyphs, names[1:]):
        outlines[name] = outline(x, y, h, bits)
        metrics[name] = width * PIXEL
        cmap[code] = name

    bold = style.lower() == "bold"
    builder = FontBuilder((ascent + descent) * PIXEL, isTTF=True)
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap(cmap)
    builder.setupGlyf(outlines)
    # The left side bearing must be the outline's left edge: browsers place
    # the glyph by it, so any other value shifts the pixels sideways.
    glyf = builder.font["glyf"]
    builder.setupHorizontalMetrics(
        {name: (advance, getattr(glyf[name], "xMin", 0)) for name, advance in metrics.items()}
    )
    builder.setupHorizontalHeader(ascent=ascent * PIXEL, descent=-descent * PIXEL)
    builder.setupNameTable({"familyName": family, "styleName": style})
    builder.setupOS2(
        sTypoAscender=ascent * PIXEL,
        sTypoDescender=-descent * PIXEL,
        sTypoLineGap=0,
        usWinAscent=ascent * PIXEL,
        usWinDescent=descent * PIXEL,
        usWeightClass=700 if bold else 400,
        fsSelection=(1 << 5 if bold else 1 << 6) | 1 << 7,
        version=4,
    )
    builder.font["OS/2"].panose.bProportion = 9  # monospaced
    builder.setupPost(isFixedPitch=1)
    # The outlines already sit on the pixel grid. A font without hinting code
    # gets FreeType's automatic hinter, which stretches some glyphs by a
    # pixel; a do-nothing control program keeps the outlines as drawn.
    prep = newTable("prep")
    prep.program = ttProgram.Program()
    prep.program.fromAssembly(["PUSHB[ ]", "0", "POP[ ]"])
    builder.font["prep"] = prep
    builder.font["head"].macStyle = 1 if bold else 0
    builder.font.flavor = "woff2"
    builder.save(target)


if __name__ == "__main__":
    main(*sys.argv[1:])
