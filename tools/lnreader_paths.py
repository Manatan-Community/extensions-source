"""Portable paths for one-way LNReader import tools."""

from __future__ import annotations

import os
from pathlib import Path


def plugins_checkout() -> Path:
    """Return the LNReader plugin checkout without embedding a user path.

    Set ``LNREADER_PLUGINS_PATH`` to use a checkout elsewhere. Otherwise the
    tools expect ``lnreader-plugins`` beside this ``extensions-source`` clone.
    """

    configured = os.environ.get("LNREADER_PLUGINS_PATH")
    checkout = (
        Path(configured).expanduser()
        if configured
        else Path(__file__).resolve().parents[2] / "lnreader-plugins"
    ).resolve()
    if not checkout.is_dir():
        raise FileNotFoundError(
            "LNReader plugin checkout not found at "
            f"{checkout}. Set LNREADER_PLUGINS_PATH to its location."
        )
    return checkout
