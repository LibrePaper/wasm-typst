// A package resolves its own imports within its own root, not the document's.
#import "util.typ": shout

#let hello(name) = shout("hello, " + name)
