// A document importing a package by the only syntax typst has for one: a
// namespace, a name and an exact version.
#import "@preview/mini:0.1.0": hello

= Packaged

#hello("world")
