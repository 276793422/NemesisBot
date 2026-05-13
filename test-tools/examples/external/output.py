#!/usr/bin/env python3
"""
NemesisBot External Channel - Output Example
Reads AI responses from stdin and processes them
"""

import sys
from datetime import datetime

def main():
    try:
        while True:
            line = sys.stdin.readline()
            if not line:
                break

            # Get current timestamp
            timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")

            # Process the AI response
            response = line.strip()

            # Output with timestamp
            print(f"[{timestamp}] AI Response:")
            print(f"  {response}")
            print()
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    main()
