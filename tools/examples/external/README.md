# External Channel Examples

This directory contains example programs for the NemesisBot external channel.

## Files

### Windows Batch Scripts

- `input.bat` - Example input program (reads from stdin, outputs to stdout)
- `output.bat` - Example output program (reads AI responses from stdin)

### Python Scripts

- `input.py` - Example input program (Python 3)
- `output.py` - Example output program with timestamps (Python 3)

## Quick Start

### Windows

1. Copy `input.bat` and `output.bat` to a location (e.g., `C:\Tools\`)

2. Configure NemesisBot:
   ```bash
   nemesisbot channel external setup
   ```

3. When prompted, enter:
   - Input exe: `C:\Tools\input.bat`
   - Output exe: `C:\Tools\output.bat`

4. Enable and start:
   ```bash
   nemesisbot channel enable external
   nemesisbot gateway
   ```

### Linux/macOS

1. Make Python scripts executable:
   ```bash
   chmod +x input.py output.py
   ```

2. Configure NemesisBot:
   ```bash
   nemesisbot channel external setup
   ```

3. When prompted, enter:
   - Input exe: `/path/to/input.py`
   - Output exe: `/path/to/output.py`

4. Enable and start:
   ```bash
   nemesisbot channel enable external
   nemesisbot gateway
   ```

## How It Works

### Input Program

The input program runs continuously and:
1. Reads user input from stdin
2. Processes/transforms it (optional)
3. Outputs to stdout for NemesisBot to read

### Output Program

The output program runs continuously and:
1. Reads AI responses from stdin
2. Processes/displays them (e.g., show notification, save to file)
3. Can also format or transform the output

## Customization

You can modify these examples to:
- Add custom formatting
- Save to database
- Show desktop notifications
- Convert formats (Markdown, HTML, etc.)
- Integrate with other systems

## Testing

Test your programs manually:

```bash
# Test input program
echo "Hello" | input.py

# Test output program
echo "AI response" | output.py
```

## See Also

- [External Channel Guide](../../docs/EXTERNAL_CHANNEL_GUIDE.md)
- [README](../../README.md)
