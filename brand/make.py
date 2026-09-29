#!/usr/bin/env python3
"""Draw this project's wordmark and icon with Glyphsmith.

    pip install git+https://github.com/Alchemy86/Glyphsmith
    python3 brand/make.py

No font is embedded, subset or traced: the letters are Glyphsmith's stroked
skeletons. The only shape that is ours is the module itself -- a ring standing
where the O of ROUNDPANEL would be, with the lit glass inside it -- and the
ruler under the word, which is the panel's four-dot column alignment.
"""

import pathlib

from glyphsmith import Mark
from glyphsmith.motifs import Motif

OUT = pathlib.Path(__file__).resolve().parent

#: The blue of a lit IPS panel, and the accent the examples draw in.
ACCENT = "#4ea3ff"


class RoundModule(Motif):
    """The round display module, standing where the O of ROUNDPANEL would be.

    The letters are centrelines stroked at 26 on a 100-unit cap grid, so the
    bezel is drawn the same way: one ring at the letters' own weight, with the
    glass filled inside it. That is what the part is -- a circular module whose
    lit area is the circle inscribed in its own square framebuffer -- and it is
    the one thing on this mark that is not the alphabet.
    """

    at = 1                  # the O of ROUNDPANEL
    cx = cy = 50
    r = 37                  # centreline: outer diameter is 37*2 + 26 = 100
    stroke = 26
    glass_r = 24            # the lit area inside the bezel

    def plot(self, replaced_advance):
        return 100          # the module is round, so it is as wide as it is tall

    def _module(self, x, y, s, palette, r, stroke, glass_r):
        return (f'<g transform="translate({x:.1f} {y:.1f}) scale({s:.4f})">\n'
                f'  <circle cx="{self.cx}" cy="{self.cy}" r="{r}" fill="none" '
                f'stroke="{palette.fg}" stroke-width="{stroke}"/>\n'
                f'  <circle cx="{self.cx}" cy="{self.cy}" r="{glass_r}" '
                f'fill="{palette.accent}"/>\n'
                f'</g>')

    def inline(self, x, cap_y, scale, palette):
        return self._module(x, cap_y, scale, palette,
                            self.r, self.stroke, self.glass_r)

    def icon(self, box, palette):
        # Redrawn heavier for the square lockup: at 16 px the wordmark's bezel
        # collapses, so the icon carries more ink and more glass.
        return self._module(0, 0, box / 100, palette, 36, 22, 27)

    band_height = 14

    #: Ticks under the word, four dots apart with every fourth one taller --
    #: the SPD2010's column alignment, which is the rule this emulator exists
    #: to make people meet before their board does.
    groups = 10

    def band(self, lay, palette):
        pitch = lay.total / (self.groups * 4 - 1)
        y = lay.cap_y + 150
        parts = []
        for i in range(self.groups * 4):
            tall = i % 4 == 0
            h = self.band_height if tall else self.band_height * 0.5
            parts.append(
                f'<rect x="{lay.x0 + i * pitch:.1f}" y="{y:.1f}" '
                f'width="{max(pitch * 0.34, 3):.1f}" height="{h:.1f}" '
                f'fill="{palette.accent if tall else palette.grey}"/>')
        return "\n".join(parts)


MARK = Mark(
    word="ROUNDPANEL",
    tagline="A 412 X 412 ROUND DISPLAY, ON YOUR DESKTOP",
    accent=ACCENT,
    motif=RoundModule(),
    scale=None, content_width=1040,
    cap_y=96, tagline_y=288, band_y=224,
    attribution="Glyphsmith (Apache-2.0)",
    regen="python3 brand/make.py",
    logo_comment="roundpanel-emu logo - the wordmark, the module in place of the O",
    icon_comment="roundpanel-emu icon - the module alone",
)

if __name__ == "__main__":
    (OUT / "roundpanel-emu-logo.svg").write_text(MARK.logo())
    (OUT / "roundpanel-emu-icon.svg").write_text(MARK.icon())
    print("wrote", OUT / "roundpanel-emu-logo.svg", OUT / "roundpanel-emu-icon.svg")
