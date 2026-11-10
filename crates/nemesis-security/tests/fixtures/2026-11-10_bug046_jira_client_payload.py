"""Read-only internal Jira client. Credentials are read from a terminal or stdin."""
import argparse
import getpass
import http.cookiejar
import json
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE_URL = "https://tbjira.lenovo.com/issue"
DEFAULT_PROXY = "http://127.0.0.1:10809"


class JiraError(Exception):
    pass


class SameOriginRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        old = urllib.parse.urlsplit(req.full_url)
        new = urllib.parse.urlsplit(newurl)
        if (old.scheme, old.netloc) != (new.scheme, new.netloc):
            raise JiraError("Cross-origin redirect refused.")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class JiraClient:
    def __init__(self, proxy=DEFAULT_PROXY, insecure=False, timeout=30):
        context = ssl.create_default_context()
        if insecure:
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
        self.timeout = timeout
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": proxy, "https": proxy}),
            urllib.request.HTTPSHandler(context=context),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
            SameOriginRedirect(),
        )

    def request(self, path, data=None, expect_json=True):
        request = urllib.request.Request(BASE_URL + path, data=data)
        request.add_header("User-Agent", "SmartConnect-Jira-ReadOnly/1.0")
        if expect_json:
            request.add_header("Accept", "application/json")
        try:
            with self.opener.open(request, timeout=self.timeout) as response:
                body = response.read()
        except urllib.error.HTTPError as exc:
            raise JiraError("Jira returned HTTP %s; no automatic login retry." % exc.code) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            # Do not reflect request details in error messages.
            raise JiraError("Network request failed; check proxy and connectivity.") from None
        if not expect_json:
            return None
        try:
            return json.loads(body.decode("utf-8"))
        except (ValueError, UnicodeError):
            raise JiraError("Expected Jira JSON; received another response, possibly a login page.") from None

    def login(self, username, password):
        self.request("/login.jsp", expect_json=False)
        form = urllib.parse.urlencode({
            "os_username": username,
            "os_password": password,
            "os_destination": "/",
            "login": "Log In",
        }).encode("utf-8")
        self.request("/login.jsp", data=form, expect_json=False)
        identity = self.request("/rest/api/2/myself")
        if not isinstance(identity, dict) or not (identity.get("name") or identity.get("key")):
            raise JiraError("Authenticated identity was not confirmed.")

    def issue(self, key):
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*-[0-9]+", key):
            raise JiraError("Invalid issue key.")
        query = urllib.parse.urlencode({"fields": "summary,status,issuetype,description,attachment,comment"})
        result = self.request("/rest/api/2/issue/" + key + "?" + query)
        if not isinstance(result, dict) or result.get("key", "").upper() != key.upper():
            raise JiraError("Response did not contain the requested issue.")
        return result


def read_credentials(from_stdin):
    if from_stdin:
        try:
            value = json.load(sys.stdin)
        except (ValueError, UnicodeError):
            raise JiraError("Credentials stdin must contain a JSON object.") from None
        if not isinstance(value, dict):
            raise JiraError("Credentials stdin must contain a JSON object.")
        username, password = value.get("username"), value.get("password")
    else:
        if not sys.stdin.isatty():
            raise JiraError("Interactive login needs a terminal; use stdin with a trusted launcher.")
        username = input("Jira username: ").strip()
        password = getpass.getpass("Jira password: ")
    if not isinstance(username, str) or not username.strip() or not isinstance(password, str) or not password:
        raise JiraError("Non-empty username and password are required.")
    return username, password


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--issue", help="Issue key, e.g. SMARTCTONE-3429")
    parser.add_argument("--proxy", default=DEFAULT_PROXY)
    parser.add_argument("--insecure", action="store_true", help="Disable TLS verification for internal access")
    parser.add_argument("--credentials-stdin", action="store_true", help="Read credential JSON from stdin, not argv")
    parser.add_argument("--output", help="Save UTF-8 JSON to a new file; never overwrites")
    args = parser.parse_args(argv)
    if args.output and not args.issue:
        parser.error("--output requires --issue")
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    try:
        username, password = read_credentials(args.credentials_stdin)
        client = JiraClient(proxy=args.proxy, insecure=args.insecure)
        client.login(username, password)
        del username, password
        result = {"authenticated": True, "base_url": BASE_URL}
        if args.issue:
            result["issue"] = client.issue(args.issue)
        text = json.dumps(result, ensure_ascii=False, indent=2)
        if args.output:
            with open(args.output, "x", encoding="utf-8") as stream:
                stream.write(text + "\n")
            print("Authentication confirmed; issue JSON saved.")
        else:
            print(text)
        return 0
    except (JiraError, OSError, EOFError, KeyboardInterrupt) as exc:
        message = str(exc) if isinstance(exc, JiraError) else "Input cancelled or output file could not be created."
        print("ERROR: " + message, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
