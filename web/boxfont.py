"""Write a companion font with the box-drawing characters akler uses, on
Monocraft's grid, which has none: the same width (6 font pixels of 120 units,
1080 per em) and one-pixel strokes. Vertical strokes reach past the line so
rows join. The page lists it right after Monocraft.

    python3 boxfont.py OUT.woff2
"""
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import newTable
from fontTools.ttLib.tables import ttProgram

EM = 1080
PIXEL = 120
ADVANCE = 6 * PIXEL
ASCENT, DESCENT = 1200, 240
# Strokes: column 2 (centred on Monocraft's five-pixel glyphs) and the row
# around the middle of the line.
X0, X1 = 2 * PIXEL, 3 * PIXEL
Y0, Y1 = 3 * PIXEL, 4 * PIXEL
# Vertical strokes overshoot the line box so stacked rows touch.
TOP, BOTTOM = ASCENT + 2 * PIXEL, -DESCENT - 2 * PIXEL


def glyph(rects):
    pen = TTGlyphPen(None)
    for left, bottom, right, top in rects:
        pen.moveTo((left, bottom))
        pen.lineTo((left, top))
        pen.lineTo((right, top))
        pen.lineTo((right, bottom))
        pen.closePath()
    return pen.glyph()


def lines(left=False, right=False, up=False, down=False, heavy=False):
    """A box character from the arms it draws from the centre."""
    y0, y1 = (Y0 - PIXEL // 2, Y1 + PIXEL // 2) if heavy else (Y0, Y1)
    rects = []
    if left or right:
        rects.append((0 if left else X0, y0, ADVANCE if right else X1, y1))
    if up or down:
        rects.append((X0, BOTTOM if down else Y0, X1, TOP if up else Y1))
    return rects


CHARACTERS = {
    "─": lines(left=True, right=True),
    "━": lines(left=True, right=True, heavy=True),
    "│": lines(up=True, down=True),
    "┌": lines(right=True, down=True),
    "┐": lines(left=True, down=True),
    "└": lines(right=True, up=True),
    "┘": lines(left=True, up=True),
    "├": lines(right=True, up=True, down=True),
    "┤": lines(left=True, up=True, down=True),
    "┬": lines(left=True, right=True, down=True),
    "┴": lines(left=True, right=True, up=True),
    "┼": lines(left=True, right=True, up=True, down=True),
    "█": [(0, BOTTOM, ADVANCE, TOP)],
    "▏": [(0, BOTTOM, PIXEL, TOP)],
}


def main(target):
    names = [".notdef"] + [f"uni{ord(ch):04X}" for ch in CHARACTERS]
    glyphs = {".notdef": glyph([])}
    glyphs.update({f"uni{ord(ch):04X}": glyph(rects) for ch, rects in CHARACTERS.items()})
    builder = FontBuilder(EM, isTTF=True)
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap({ord(ch): f"uni{ord(ch):04X}" for ch in CHARACTERS})
    builder.setupGlyf(glyphs)
    glyf = builder.font["glyf"]
    builder.setupHorizontalMetrics(
        {name: (ADVANCE, getattr(glyf[name], "xMin", 0)) for name in names}
    )
    builder.setupHorizontalHeader(ascent=ASCENT, descent=-DESCENT)
    builder.setupNameTable({"familyName": "Monocraft Box", "styleName": "Regular"})
    builder.setupOS2(
        sTypoAscender=ASCENT,
        sTypoDescender=-DESCENT,
        sTypoLineGap=0,
        usWinAscent=ASCENT,
        usWinDescent=DESCENT,
        fsSelection=1 << 6 | 1 << 7,
        version=4,
    )
    builder.font["OS/2"].panose.bProportion = 9  # monospaced
    builder.setupPost(isFixedPitch=1)
    # Keep FreeType's automatic hinter from moving strokes (see bdf2font.py).
    prep = newTable("prep")
    prep.program = ttProgram.Program()
    prep.program.fromAssembly(["PUSHB[ ]", "0", "POP[ ]"])
    builder.font["prep"] = prep
    builder.font.flavor = "woff2"
    builder.save(target)


if __name__ == "__main__":
    main(*sys.argv[1:])
