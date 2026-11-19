"""Portable read-only Jira client. Credentials come from environment injection or stdin."""
import argparse
import base64
import getpass
import hashlib
from datetime import datetime, timezone
from pathlib import Path
import http.cookiejar
import http.client
import json
import os
import re
import ssl
import stat
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class JiraError(Exception):
    pass


class AttachmentError(JiraError):
    def __init__(self, code, message, retryable=False, http_status=None):
        super().__init__(message)
        self.code = code
        self.retryable = retryable
        self.http_status = http_status


def contained_path(root, path):
    root, path = Path(root).resolve(), Path(path).absolute()
    try:
        relative = path.relative_to(root)
        path.resolve().relative_to(root)
    except ValueError:
        raise JiraError('Archive path leaves the authorized root.') from None
    current = root
    for part in relative.parts:
        current = current / part
        try:
            info = current.lstat()
        except FileNotFoundError:
            continue
        # lstat exposes Windows reparse points even without Path.is_junction().
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
            raise JiraError('Archive descendants must not be directory links.')
    return path


def publish_new_file(source, target):
    # Windows rename refuses an existing target; Unix rename would overwrite it.
    if os.name == 'nt':
        os.rename(source, target)
    else:
        os.link(source, target)


def normalize_base_url(value):
    parts = urllib.parse.urlsplit(value)
    if parts.scheme != 'https' or not parts.hostname or parts.username is not None or parts.password is not None:
        raise JiraError('Provide an HTTPS Jira base URL without credentials.')
    if parts.query or parts.fragment or re.search(r'/(browse|rest)/', parts.path):
        raise JiraError('Provide the base URL including its context path, not an issue or API URL.')
    return value.rstrip('/')


def origin(url):
    parts = urllib.parse.urlsplit(url)
    return parts.scheme, parts.hostname, parts.port or 443


class SameOriginRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if origin(req.full_url) != origin(newurl):
            raise JiraError('Cross-origin redirect refused.')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class JiraClient:
    def __init__(self, base_url, proxy=None, insecure=False, timeout=30, api_version='2'):
        self.base_url = normalize_base_url(base_url)
        if api_version not in ('2', '3'):
            raise JiraError('API version must be 2 or 3.')
        self.api = '/rest/api/' + api_version
        context = ssl.create_default_context()
        if insecure:
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
        self.timeout = timeout
        self.authorization = None
        # Without an explicit proxy, leave the platform proxy configuration alone.
        handlers = [urllib.request.HTTPSHandler(context=context),
                    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
                    SameOriginRedirect()]
        if proxy is not None:
            if urllib.parse.urlsplit(proxy).scheme not in ('http', 'https'):
                raise JiraError('Proxy must be an HTTP(S) URL.')
            handlers.append(urllib.request.ProxyHandler({'http': proxy, 'https': proxy}))
        self.opener = urllib.request.build_opener(*handlers)

    def request(self, path, data=None, expect_json=True):
        if not path.startswith('/') or path.startswith('//'):
            raise JiraError('Expected a relative Jira request path.')
        if data is not None and path != '/login.jsp':
            raise JiraError('Business write requests are not allowed.')
        req = urllib.request.Request(self.base_url + path, data=data)
        req.add_header('User-Agent', 'Personal-Jira-ReadOnly/2.0')
        if self.authorization:
            req.add_header('Authorization', self.authorization)
        if expect_json:
            req.add_header('Accept', 'application/json')
        try:
            with self.opener.open(req, timeout=self.timeout) as response:
                body = response.read()
        except urllib.error.HTTPError as exc:
            raise JiraError('HTTP %s; no automatic authentication retry.' % exc.code) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise JiraError('Network/TLS request failed; confirm access requirements with the user.') from None
        if not expect_json:
            return body.decode('utf-8', errors='replace')
        try:
            return json.loads(body.decode('utf-8'))
        except (ValueError, UnicodeError):
            raise JiraError('Expected JSON; response may be a login page.') from None

    def login(self, username=None, password=None, auth='form', token=None):
        required = (username, password) if auth == 'form' else ((username, token) if auth == 'basic' else (token,))
        for value in required:
            if not isinstance(value, str) or not value.strip() or value.startswith(('vault:', '{{vault:')):
                raise JiraError('Missing or unresolved authentication input.')
        self.authorization = None
        if auth == 'form':
            if not username or not password:
                raise JiraError('Form authentication needs username and password.')
            self.request('/login.jsp', expect_json=False)
            form = urllib.parse.urlencode({'os_username': username, 'os_password': password,
                                          'os_destination': '/', 'login': 'Log In'}).encode('utf-8')
            self.request('/login.jsp', data=form, expect_json=False)
        elif auth == 'basic':
            if not username or not token:
                raise JiraError('Basic authentication needs a username and an authorized token.')
            encoded = base64.b64encode((username + ':' + token).encode('utf-8')).decode('ascii')
            self.authorization = 'Basic ' + encoded
        elif auth == 'bearer':
            if not token:
                raise JiraError('Bearer authentication needs an authorized token.')
            self.authorization = 'Bearer ' + token
        else:
            raise JiraError('Unsupported authentication method.')
        identity = self.request(self.api + '/myself')
        if not isinstance(identity, dict) or not any(identity.get(k) for k in ('accountId', 'name', 'key')):
            raise JiraError('Authenticated identity was not confirmed.')
        return identity

    def issue(self, key):
        if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*-[0-9]+', key):
            raise JiraError('Invalid issue key.')
        result = self.request(self.api + '/issue/' + key + '?fields=*all&expand=names')
        if not isinstance(result, dict) or result.get('key') != key:
            raise JiraError('Cloud issue key does not exactly match the requested key, including case.')
        return result

    def comments(self, key):
        if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*-[0-9]+', key):
            raise JiraError('Invalid issue key.')
        items, seen, total = [], set(), None
        while total is None or len(items) < total:
            query = urllib.parse.urlencode({'startAt': len(items), 'maxResults': 100})
            page = self.request(self.api + '/issue/' + key + '/comment?' + query)
            if not isinstance(page, dict):
                raise JiraError('Invalid comment page.')
            count, batch = page.get('total'), page.get('comments')
            if type(count) is not int or count < 0 or not isinstance(batch, list):
                raise JiraError('Invalid comment pagination metadata.')
            if total is not None and total != count:
                raise JiraError('Comment total changed; snapshot is incomplete.')
            total = count
            if page.get('startAt', len(items)) != len(items):
                raise JiraError('Unexpected comment page offset.')
            if not batch and len(items) < total:
                raise JiraError('Premature empty comment page.')
            for comment in batch:
                identifier = comment.get('id') if isinstance(comment, dict) else None
                if identifier is None or str(identifier) in seen:
                    raise JiraError('Missing or duplicate comment ID.')
                seen.add(str(identifier))
                items.append(comment)
            if len(items) > total:
                raise JiraError('Comment count exceeds total.')
        return {'comments': items, 'total': total}

    def download_attachment(self, metadata, directory, archive_root=None):
        result = {'id': metadata.get('id'), 'filename': metadata.get('filename'),
                  'expected_size': metadata.get('size'), 'size': 0,
                  'mime_type': metadata.get('mimeType'), 'status': 'failed',
                  'path': None, 'sha256': None, 'attempts': 0}
        try:
            url = metadata.get('content', '')
            parts = urllib.parse.urlsplit(url)
            if parts.username is not None or parts.password is not None or origin(url) != origin(self.base_url):
                raise JiraError('Attachment must use the Jira HTTPS origin.')
            expected = metadata.get('size')
            if type(expected) is not int or expected < 0:
                raise JiraError('Missing or invalid attachment size.')
            identifier = str(metadata.get('id', ''))
            if not re.fullmatch(r'[0-9]+', identifier):
                raise JiraError('Invalid attachment ID.')
            directory = Path(directory).absolute()
            root = Path(archive_root).resolve() if archive_root is not None else directory.parent.resolve()
            folder = contained_path(root, directory)
            folder.mkdir(parents=True, exist_ok=True)
            target = contained_path(root, folder / (identifier + '_' + safe_filename(metadata.get('filename', 'attachment'))))
            if target.exists():
                raise JiraError('Attachment already exists; refusing overwrite.')
        except (JiraError, OSError, ValueError, TypeError):
            result.update(error_code='destination_or_metadata',
                          error='Invalid attachment origin, metadata or destination; no request sent.')
            return result

        for attempt in range(1, 4):
            partial = None
            result.update(attempts=attempt, size=0, path=None, sha256=None)
            try:
                contained_path(root, folder)
                req = urllib.request.Request(url)
                if self.authorization:
                    req.add_header('Authorization', self.authorization)
                digest = hashlib.sha256()
                with self.opener.open(req, timeout=self.timeout) as response:
                    final = urllib.parse.urlsplit(response.geturl())
                    if origin(response.geturl()) != origin(self.base_url):
                        raise AttachmentError('origin', 'Cross-origin attachment response refused.')
                    context = urllib.parse.urlsplit(self.base_url).path.rstrip('/')
                    if final.path.rstrip('/') in {context + suffix for suffix in ('/login.jsp', '/login', '/login.action')}:
                        raise AttachmentError('authentication', 'Attachment redirected to a login page.')
                    content_type = response.headers.get('Content-Type', '').split(';', 1)[0].strip().lower()
                    html_types = ('text/html', 'application/xhtml+xml')
                    filename = str(metadata.get('filename', '')).lower()
                    declared_type = str(metadata.get('mimeType', '')).split(';', 1)[0].strip().lower()
                    if content_type in html_types and declared_type not in html_types and not filename.endswith(('.html', '.htm', '.xhtml')):
                        raise AttachmentError('unexpected_html', 'Unexpected HTML response for a non-HTML attachment.')
                    # Unique names isolate retries from stale or interrupted downloads.
                    contained_path(root, folder)
                    descriptor, name = tempfile.mkstemp(prefix=identifier + '-', suffix='.partial', dir=folder)
                    partial = Path(name)
                    result['path'] = partial.name
                    with os.fdopen(descriptor, 'wb') as stream:
                        while True:
                            block = response.read(65536)
                            if not block:
                                break
                            stream.write(block)
                            digest.update(block)
                            result['size'] += len(block)
                            if result['size'] > expected:
                                raise AttachmentError('size', 'Attachment exceeds declared size.', retryable=True)
                result['sha256'] = digest.hexdigest()
                if result['size'] != expected:
                    raise AttachmentError('size', 'Attachment size mismatch.', retryable=True)
                contained_path(root, partial)
                contained_path(root, target)
                publish_new_file(partial, target)
                result.update(status='downloaded', path=target.name)
                if partial.exists():
                    try:
                        partial.unlink()
                    except OSError:
                        result['cleanup_warning'] = 'Verified attachment saved; temporary hard link could not be removed.'
                return result
            except urllib.error.HTTPError as exc:
                error = AttachmentError('http', 'Attachment HTTP request failed.',
                                        retryable=exc.code in (408, 429, 500, 502, 503, 504), http_status=exc.code)
            except (urllib.error.URLError, TimeoutError, ConnectionError, ssl.SSLError,
                    http.client.IncompleteRead, http.client.RemoteDisconnected) as exc:
                reason = exc.reason if isinstance(exc, urllib.error.URLError) else exc
                if isinstance(reason, ssl.SSLCertVerificationError):
                    error = AttachmentError('tls', 'TLS verification failed; no automatic downgrade.')
                elif isinstance(reason, ssl.SSLError) and not isinstance(reason, (ssl.SSLEOFError, ssl.SSLZeroReturnError)):
                    error = AttachmentError('tls', 'TLS protocol failed; no automatic downgrade.')
                else:
                    error = AttachmentError('network', 'Attachment transfer interrupted.', retryable=True)
            except AttachmentError as exc:
                error = exc
            except (JiraError, OSError, ValueError, TypeError):
                error = AttachmentError('destination', 'Attachment destination or safe publication failed.')
            if not error.retryable or attempt == 3:
                result.update(error_code=error.code, error=str(error), http_status=error.http_status)
                return result
            # Delete only this attempt's newly-created incomplete file, never old user files.
            if partial is not None and partial.exists():
                try:
                    partial.unlink()
                except OSError:
                    result.update(error_code='cleanup', error='Cannot remove this attempt temporary file; retry stopped.')
                    return result
            time.sleep(attempt)
        return result

    def archive_issue(self, key, archive_root):
        if not archive_root:
            raise JiraError('An explicit archive root is required.')
        issue = self.issue(key)
        comments = self.comments(key)
        root = Path(archive_root).resolve()
        root.mkdir(parents=True, exist_ok=True)
        folder = contained_path(root, root / key)
        if folder.exists() and key not in {entry.name for entry in root.iterdir()}:
            raise JiraError('Local issue directory case conflicts with the exact cloud issue key.')
        try:
            folder.mkdir()
        except FileExistsError:
            folder = contained_path(root, folder / ('snapshot-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')))
            folder.mkdir()
        manifest = []
        for attachment in issue.get('fields', {}).get('attachment', []):
            entry = self.download_attachment(attachment, folder / 'attachments', archive_root=root)
            if entry['path']:
                entry['path'] = 'attachments/' + entry['path']
            manifest.append(entry)
        contained_path(root, folder)
        for name, value in (('issue.json', issue), ('comments.json', comments),
                            ('attachments-manifest.json', manifest)):
            with (folder / name).open('x', encoding='utf-8') as stream:
                json.dump(value, stream, ensure_ascii=False, indent=2)
                stream.write('\n')
        succeeded = sum(entry['status'] == 'downloaded' for entry in manifest)
        summary = ('# ' + key + '\n\nCaptured: ' + datetime.now(timezone.utc).isoformat()
                   + '\nSource: ' + self.base_url + '\nComments: ' + str(comments['total'])
                   + '\nAttachments: ' + str(succeeded) + ' downloaded, '
                   + str(len(manifest) - succeeded) + ' failed\n\n'
                   + 'Comment repair claims are not verified fixes. Only visible fields and direct attachments are included.\n\n'
                   + '## Fields (original structured values)\n\n```json\n'
                   + json.dumps(issue.get('fields', {}), ensure_ascii=False, indent=2)
                   + '\n```\n\n## Comments\n\n```json\n'
                   + json.dumps(comments, ensure_ascii=False, indent=2) + '\n```\n')
        with (folder / 'README.md').open('x', encoding='utf-8') as stream:
            stream.write(summary)
        return folder

    def browse(self, jql, anchor=None):
        if anchor and not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*-[0-9]+', anchor):
            raise JiraError('Invalid anchor issue key.')
        path = '/browse/' + anchor if anchor else '/issues/'
        return self.request(path + '?' + urllib.parse.urlencode({'jql': jql}), expect_json=False)


def safe_filename(value):
    value = re.sub(r'[<>:"/\\|?*\x00-\x1f]', '_', str(value))[:140].rstrip(' .')
    return value or 'attachment'


def read_credentials(args):
    if args.credentials_stdin:
        try:
            values = json.load(sys.stdin)
        except (ValueError, UnicodeError):
            raise JiraError('Expected credential JSON on stdin.') from None
        if not isinstance(values, dict):
            raise JiraError('Expected a credential object.')
    else:
        values = {'username': os.environ.get('JIRA_USERNAME'),
                  'password': os.environ.get('JIRA_PASSWORD'), 'token': os.environ.get('JIRA_TOKEN')}
    needed = ('username', 'password') if args.auth == 'form' else (('username', 'token') if args.auth == 'basic' else ('token',))
    for key in needed:
        if not values.get(key) and sys.stdin.isatty() and not args.credentials_stdin:
            values[key] = input('Jira username: ') if key == 'username' else getpass.getpass('Jira ' + key + ': ')
        if not isinstance(values.get(key), str) or not values[key] or values[key].startswith(('vault:', '{{vault:')):
            raise JiraError('Missing or unresolved credential: ' + key + '. Use authorized environment injection or stdin.')
    return {key: values.get(key) for key in ('username', 'password', 'token')}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-url', required=True, help='Jira base URL including any context path')
    parser.add_argument('--auth', required=True, choices=('form', 'basic', 'bearer'))
    parser.add_argument('--api-version', choices=('2', '3'), default='2')
    parser.add_argument('--proxy', help='Only set when explicitly provided by the user')
    parser.add_argument('--insecure', action='store_true', help='Disable TLS verification only with explicit authorization')
    parser.add_argument('--issue')
    parser.add_argument('--work-root', help='Explicit user-provided working directory')
    parser.add_argument('--archive-root', help='Archive directory, relative to --work-root if not absolute')
    parser.add_argument('--credentials-stdin', action='store_true')
    parser.add_argument('--output', help='New UTF-8 JSON file; never overwrites')
    args = parser.parse_args(argv)
    if args.output and not args.issue:
        parser.error('--output requires --issue')
    if args.archive_root and (not args.issue or not args.work_root or args.output):
        parser.error('--archive-root requires --issue and --work-root, and excludes --output')
    try:
        client = JiraClient(args.base_url, proxy=args.proxy, insecure=args.insecure, api_version=args.api_version)
        credentials = read_credentials(args)
        client.login(auth=args.auth, **credentials)
        del credentials
        result = {'authenticated': True, 'base_url': client.base_url}
        if args.archive_root:
            work_root = Path(args.work_root).resolve()
            if not work_root.is_dir():
                raise JiraError('Working root must be an existing user-provided directory.')
            destination = Path(args.archive_root)
            if not destination.is_absolute():
                destination = work_root / destination
            result['archive_path'] = str(client.archive_issue(args.issue, destination))
        elif args.issue:
            result['issue'] = client.issue(args.issue)
        text = json.dumps(result, ensure_ascii=False, indent=2)
        if args.output:
            with open(args.output, 'x', encoding='utf-8') as stream:
                stream.write(text + '\n')
            print('Authentication confirmed; issue JSON saved.')
        else:
            print(text)
        return 0
    except (JiraError, OSError, EOFError, KeyboardInterrupt) as exc:
        print('ERROR: ' + (str(exc) if isinstance(exc, JiraError) else 'Input/output failed or was cancelled.'), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
