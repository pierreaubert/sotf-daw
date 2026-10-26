"""Qualify actual spectral restoration output against classical compression."""

import argparse
from pathlib import Path

from evaluate_expander_mask import evaluate
from spectral_restoration_native import parameters, render


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    evaluate(
        args.renderer,
        args.model,
        args.output,
        native_render=render,
        parameter_factory=parameters,
        seed_first=20289120,
        consumer="spectral_compressor_restoration",
    )


if __name__ == "__main__":
    main()
