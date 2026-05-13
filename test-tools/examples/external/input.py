#!/usr/bin/env python3
"""
NemesisBot External Channel - Input Example
Reads from stdin and outputs to stdout
"""

import sys

def main():
    try:
        while True:
            line = sys.stdin.readline()
            if not line:
                break
            # Strip newline and output
            cleaned = line.strip()
            if cleaned:
                print(cleaned)
                sys.stdout.flush()
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    main()
