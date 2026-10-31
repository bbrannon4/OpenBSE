# Preface {.unnumbered}

This is the Engineering Reference for [OpenBSE](https://github.com/bbrannon4/OpenBSE),
an open-source building energy simulation engine. It describes the
algorithms, equations and physics models the engine implements: what each
model computes, how it is solved, and where the method comes from.
@sec-intro explains its conventions and how a model section is organized.

The reference is written in Markdown with LaTeX mathematics, one file per
chapter, in `docs/engineering-reference/` of the repository. The same source
builds this website and a PDF (the download link in the sidebar). Each file
can also be read directly on GitHub.

**Building it locally.** With [Quarto](https://quarto.org) and a LaTeX
distribution installed (`quarto install tinytex`), run `quarto render` in
`docs/engineering-reference/`; the website and PDF are written to `_book/`.
Quarto and LaTeX are needed only to build the documentation. They are not
dependencies of OpenBSE.

**Status.** The reference was written chapter by chapter from the code under
issue [#181](https://github.com/bbrannon4/OpenBSE/issues/181). See @sec-status.
