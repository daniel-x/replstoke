#!/usr/bin/env python3

import sys
import os


def printflush(stream, s: str):
    stream.write(s)
    stream.flush()


def println_ef(s: str, end="\n"):
    printflush(sys.stderr, f"dummyrepl pid={os.getpid()} on stderr: {s}{end}")


def println_of(s: str, end="\n"):
    printflush(sys.stdout, f"dummyrepl pid={os.getpid()} on stdout: {s}{end}")


def main():
    println_ef("##### started")

    i = 0

    println_of(f"{i=}: welcome, this is the dummyrepl")

    while True:
        i += 1
        try:
            l = next(sys.stdin)
        except StopIteration:
            println_ef(f"{i=}: EOF on sys.stdin, exiting")
            break

        l = l.strip()

        println_ef(f"{i=}: input from client: {l}")

        try:
            println_of(f"{i=}: input_from_client={l.strip()}, printing a blank line hereafter:\n\n", end="")
        except BrokenPipeError:
            println_ef("{i=}: broken pipe when writing to stdout")


if __name__ == "__main__":
    try:
        main()
    except BaseException as e:
        println_ef(f"exception: {type(e).__name__}")
        raise
    finally:
        println_ef("##### exiting")

