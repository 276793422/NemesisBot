---
name: test-skill
description: A simple test skill that performs basic math calculations and string operations. Use this skill when you need to demonstrate that skills are being loaded and executed correctly.
---

# Test Skill

This is a simple test skill to verify that the skills system is working correctly.

## Capabilities

### 1. Basic Math

The agent can perform basic mathematical operations:
- Addition: 5 + 3 = 8
- Multiplication: 4 * 7 = 28
- Division: 20 / 4 = 5

### 2. String Operations

The agent can manipulate strings:
- Convert to uppercase: "hello" → "HELLO"
- Convert to lowercase: "WORLD" → "world"
- Reverse text: "test" → "tset"

### 3. Current Time

The agent can report the current timestamp when this skill is active.

## Test Instructions

When a user asks to "test the skill" or "verify skills are working":

1. Acknowledge that the test-skill is active
2. Perform a simple calculation (e.g., 123 + 456)
3. Transform a string (e.g., convert "Skill Test" to uppercase)
4. Confirm the skill is functioning correctly

Example response:
```
✅ Test Skill is ACTIVE!

Calculation: 123 + 456 = 579
String transformation: "Skill Test" → "SKILL TEST"

The skills system is working correctly!
```
