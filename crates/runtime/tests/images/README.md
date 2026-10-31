The sample files are generated 4×3 images with RGB (32, 128, 224). sample.ico is
16×12, because macOS ImageIO skips icons narrower than 16 pixels.
alpha.avif has alpha 85; the other samples are opaque. gray.jpg and cmyk.jpg
use the same color, because zune-jpeg decodes them without its BGRA output.

corrupt-base.avif comes from avif-decode 3.0.0's tests/data/tiny.avif.
Its BSD-3-Clause license is in AVIF-LICENSE. The regression test flips byte 280
to check that a damaged AV1 frame returns an error with rav1d. ImageIO draws it
as transparent pixels.
