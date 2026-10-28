The sample files are generated 4×3 images with RGB (32, 128, 224).
alpha.avif has alpha 85; the other samples are opaque.

corrupt-base.avif comes from avif-decode 3.0.0's tests/data/tiny.avif.
Its BSD-3-Clause license is in AVIF-LICENSE. The regression test flips byte 280
to check that a damaged AV1 frame returns an error.
