"""Offline tests. Reserved example domains and synthetic runtime inputs only."""
import hashlib
import importlib.util
import io
import json
import os
import subprocess
from types import SimpleNamespace
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import urllib.error
import urllib.request

spec = importlib.util.spec_from_file_location('jira_client', Path(__file__).resolve().parents[1] / 'scripts' / 'jira_client.py')
jira = importlib.util.module_from_spec(spec)
spec.loader.exec_module(jira)
BASE = 'https://jira.example.invalid/context'


class Response(io.BytesIO):
    def __init__(self, body, content_type='application/octet-stream'):
        super().__init__(body)
        self.headers = {'Content-Type': content_type}

    def geturl(self):
        return BASE + '/attachment/1'


class Tests(unittest.TestCase):
    def setUp(self):
        sleep = patch.object(jira.time, 'sleep')
        self.sleep = sleep.start()
        self.addCleanup(sleep.stop)

    def client(self):
        client = jira.JiraClient(BASE)
        client.opener = Mock()
        return client

    def metadata(self, body):
        return {'id': '1', 'filename': '../日志.bin', 'size': len(body),
                'content': BASE + '/attachment/1'}

    def test_context_and_url_validation(self):
        self.assertEqual(jira.normalize_base_url(BASE + '/'), BASE)
        for value in ('http://example.invalid', BASE + '?x=1', BASE + '/browse/TEST-1'):
            with self.assertRaises(jira.JiraError):
                jira.normalize_base_url(value)

    def test_proxy_default_and_explicit(self):
        with patch.object(jira.urllib.request, 'build_opener') as build:
            jira.JiraClient(BASE)
            self.assertFalse(any(isinstance(x, urllib.request.ProxyHandler) for x in build.call_args.args))
            jira.JiraClient(BASE, proxy='http://proxy.example.invalid:8080')
            handlers = [x for x in build.call_args.args if isinstance(x, urllib.request.ProxyHandler)]
            self.assertEqual(handlers[0].proxies['https'], 'http://proxy.example.invalid:8080')

    def test_auth_modes(self):
        for mode in ('form', 'basic', 'bearer'):
            c = self.client()
            c.request = Mock(return_value={'accountId': 'synthetic'})
            values = dict.fromkeys(('username', 'password', 'token'), ''.join(['synthetic', '-input']))
            self.assertEqual(c.login(auth=mode, **values)['accountId'], 'synthetic')
            self.assertEqual(c.request.call_args.args[0], '/rest/api/2/myself')
            self.assertEqual(c.request.call_count, 3 if mode == 'form' else 1)

    def test_missing_credentials_rejected_before_network(self):
        c = self.client()
        with self.assertRaises(jira.JiraError):
            c.login()
        c.opener.open.assert_not_called()

    def test_identity_not_confirmed(self):
        c = self.client()
        c.request = Mock(return_value={})
        with self.assertRaises(jira.JiraError):
            c.login(auth='bearer', **dict.fromkeys(['token'], 'synthetic-input'))

    def test_json_chinese_and_context(self):
        c = self.client()
        c.opener.open.return_value = Response(json.dumps({'title': '中文'}, ensure_ascii=False).encode())
        self.assertEqual(c.request('/rest/api/2/myself')['title'], '中文')
        self.assertEqual(c.opener.open.call_args.args[0].full_url, BASE + '/rest/api/2/myself')

    def test_html_and_403_fail_without_retry(self):
        c = self.client()
        c.opener.open.return_value = Response(b'<html>login</html>')
        with self.assertRaises(jira.JiraError):
            c.request('/rest/api/2/myself')
        c.opener.open.reset_mock()
        c.opener.open.side_effect = urllib.error.HTTPError(BASE, 403, 'Forbidden', {}, None)
        with self.assertRaisesRegex(jira.JiraError, '403'):
            c.request('/rest/api/2/myself')
        self.assertEqual(c.opener.open.call_count, 1)

    def test_business_write_rejected(self):
        c = self.client()
        with self.assertRaises(jira.JiraError):
            c.request('/rest/api/2/issue/TEST-1', data=b'{}')
        c.opener.open.assert_not_called()

    def test_full_issue_and_invalid_key(self):
        c = self.client()
        c.request = Mock(return_value={'key': 'TEST-1', 'fields': {}})
        c.issue('TEST-1')
        self.assertIn('fields=*all&expand=names', c.request.call_args.args[0])
        with self.assertRaises(jira.JiraError):
            c.issue('../TEST-1')

    def test_browse_encodes_jql(self):
        c = self.client()
        c.request = Mock(return_value='results')
        c.browse('summary ~ "中文"', 'TEST-1')
        self.assertIn('/browse/TEST-1?jql=', c.request.call_args.args[0])
        self.assertNotIn('中文', c.request.call_args.args[0])

    def test_cross_origin_redirect(self):
        with self.assertRaises(jira.JiraError):
            jira.SameOriginRedirect().redirect_request(urllib.request.Request(BASE), None, 302, '', {}, 'https://other.example.invalid/')

    def test_comment_two_pages(self):
        c = self.client()
        c.request = Mock(side_effect=[{'total': 2, 'startAt': 0, 'comments': [{'id': '1'}]},
                                     {'total': 2, 'startAt': 1, 'comments': [{'id': '2'}]}])
        self.assertEqual(c.comments('TEST-1')['total'], 2)
        self.assertIn('startAt=1', c.request.call_args.args[0])

    def test_comment_incomplete_and_duplicate(self):
        for second in ([], [{'id': '1'}]):
            c = self.client()
            c.request = Mock(side_effect=[{'total': 2, 'comments': [{'id': '1'}]},
                                         {'total': 2, 'comments': second}])
            with self.assertRaises(jira.JiraError):
                c.comments('TEST-1')

    def test_binary_download_hash_and_no_overwrite(self):
        c = self.client()
        body = bytes(range(256)) * 300
        c.opener.open.return_value = Response(body)
        with tempfile.TemporaryDirectory() as folder:
            result = c.download_attachment(self.metadata(body), folder)
            self.assertEqual(result['status'], 'downloaded')
            self.assertEqual(result['sha256'], hashlib.sha256(body).hexdigest())
            self.assertEqual((Path(folder) / result['path']).read_bytes(), body)
            self.assertNotIn('/', result['path'])
            self.assertEqual(c.download_attachment(self.metadata(body), folder)['status'], 'failed')

    def test_attachment_failures(self):
        for reason in ('cross-origin', 'short', 'html'):
            c = self.client()
            meta = self.metadata(b'abc')
            if reason == 'cross-origin':
                meta['content'] = 'https://other.example.invalid/file'
            c.opener.open.side_effect = [Response(b'a', 'text/html' if reason == 'html' else 'application/octet-stream') for _ in range(3)]
            with tempfile.TemporaryDirectory() as folder:
                result = c.download_attachment(meta, folder)
                self.assertEqual(result['status'], 'failed')
            self.assertEqual(c.opener.open.call_count, 0 if reason == 'cross-origin' else (3 if reason == 'short' else 1))

    def test_exact_cloud_issue_key(self):
        c = self.client()
        c.request = Mock(return_value={'key': 'TEST-1', 'fields': {}})
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaisesRegex(jira.JiraError, 'exactly match'):
                c.archive_issue('test-1', folder)
            self.assertEqual(list(Path(folder).iterdir()), [])
        c.request.return_value = {'key': 'test-1', 'fields': {}}
        self.assertEqual(c.issue('test-1')['key'], 'test-1')

    def test_html_and_xhtml_attachments(self):
        for mime, filename in (('text/html', 'report.html'), ('application/xhtml+xml', 'report.xhtml'),
                               ('text/html', 'report.bin')):
            c = self.client()
            body = b'<html><form>diagnostic report</form></html>'
            meta = self.metadata(body)
            meta.update(filename=filename, mimeType=mime)
            c.opener.open.return_value = Response(body, mime)
            with tempfile.TemporaryDirectory() as folder:
                result = c.download_attachment(meta, folder)
                self.assertEqual(result['status'], 'downloaded')
                self.assertEqual((Path(folder) / result['path']).read_bytes(), body)

    def test_login_redirect_rejected_even_for_html(self):
        c = self.client()
        body = b'<html>login</html>'
        meta = self.metadata(body)
        meta.update(filename='report.html', mimeType='text/html')
        response = Response(body, 'text/html')
        response.geturl = lambda: BASE + '/login.jsp'
        c.opener.open.return_value = response
        with tempfile.TemporaryDirectory() as folder:
            result = c.download_attachment(meta, folder)
            self.assertEqual(result['error_code'], 'authentication')
            self.assertEqual(list(Path(folder).iterdir()), [])
        self.assertEqual(c.opener.open.call_count, 1)

    def test_retry_after_short_read_ignores_stale_partial(self):
        c = self.client()
        body = b'complete'
        c.opener.open.side_effect = [Response(b'bad'), Response(body)]
        with tempfile.TemporaryDirectory() as folder:
            stale = Path(folder) / '1_old.partial'
            stale.write_bytes(b'old interrupted download')
            result = c.download_attachment(self.metadata(body), folder)
            self.assertEqual(result['status'], 'downloaded')
            self.assertEqual(result['attempts'], 2)
            self.assertEqual(result['size'], len(body))
            self.assertEqual(result['sha256'], hashlib.sha256(body).hexdigest())
            self.assertEqual(stale.read_bytes(), b'old interrupted download')
            self.assertEqual(len(list(Path(folder).glob('*.partial'))), 1)
        self.sleep.assert_called_once_with(1)

    def test_retry_exhaustion_keeps_only_last_partial(self):
        c = self.client()
        c.opener.open.side_effect = [Response(b'a') for _ in range(3)]
        with tempfile.TemporaryDirectory() as folder:
            result = c.download_attachment(self.metadata(b'abc'), folder)
            self.assertEqual(result['attempts'], 3)
            self.assertEqual(result['error_code'], 'size')
            self.assertEqual(len(list(Path(folder).iterdir())), 1)
            self.assertTrue(result['path'].endswith('.partial'))
        self.assertEqual([call.args[0] for call in self.sleep.call_args_list], [1, 2])

    def test_retry_transient_http_and_network(self):
        for error in (urllib.error.HTTPError(BASE, 503, 'Unavailable', {}, None),
                      urllib.error.URLError('interrupted'), TimeoutError()):
            c = self.client()
            c.opener.open.side_effect = [error, Response(b'abc')]
            with tempfile.TemporaryDirectory() as folder:
                result = c.download_attachment(self.metadata(b'abc'), folder)
                self.assertEqual(result['status'], 'downloaded')
                self.assertEqual(result['attempts'], 2)

    def test_auth_and_tls_errors_not_retried(self):
        for error in (urllib.error.HTTPError(BASE, 401, 'Unauthorized', {}, None),
                      urllib.error.HTTPError(BASE, 403, 'Forbidden', {}, None),
                      urllib.error.URLError(jira.ssl.SSLCertVerificationError('untrusted'))):
            c = self.client()
            c.opener.open.side_effect = error
            with tempfile.TemporaryDirectory() as folder:
                result = c.download_attachment(self.metadata(b'abc'), folder)
                self.assertEqual(result['status'], 'failed')
                self.assertEqual(result['attempts'], 1)
            self.assertEqual(c.opener.open.call_count, 1)

    def test_tls_stream_interruptions_retry(self):
        for error_type in (jira.ssl.SSLEOFError, jira.ssl.SSLZeroReturnError):
            for wrapped in (False, True):
                with self.subTest(error=error_type.__name__, wrapped=wrapped):
                    c = self.client()
                    error = error_type('stream interrupted')
                    if wrapped:
                        error = urllib.error.URLError(error)
                    c.opener.open.side_effect = [error, Response(b'abc')]
                    with tempfile.TemporaryDirectory() as folder:
                        result = c.download_attachment(self.metadata(b'abc'), folder)
                        self.assertEqual(result['status'], 'downloaded')
                        self.assertEqual(result['attempts'], 2)
                        self.assertEqual((Path(folder) / result['path']).read_bytes(), b'abc')

    def test_tls_stream_read_failure_retries_from_new_file(self):
        c = self.client()
        interrupted = Response(b'ab')
        interrupted.read = Mock(side_effect=[b'ab', jira.ssl.SSLEOFError('stream ended')])
        c.opener.open.side_effect = [interrupted, Response(b'abc')]
        with tempfile.TemporaryDirectory() as folder:
            result = c.download_attachment(self.metadata(b'abc'), folder)
            self.assertEqual(result['status'], 'downloaded')
            self.assertEqual(result['attempts'], 2)
            self.assertEqual(result['size'], 3)
            self.assertEqual(result['sha256'], hashlib.sha256(b'abc').hexdigest())
            self.assertEqual(list(Path(folder).glob('*.partial')), [])
            self.assertEqual((Path(folder) / result['path']).read_bytes(), b'abc')

    def test_tls_certificate_and_protocol_errors_never_retry(self):
        for error_type in (jira.ssl.SSLCertVerificationError, jira.ssl.SSLError):
            for wrapped in (False, True):
                with self.subTest(error=error_type.__name__, wrapped=wrapped):
                    c = self.client()
                    error = error_type('TLS rejected')
                    if wrapped:
                        error = urllib.error.URLError(error)
                    c.opener.open.side_effect = error
                    with tempfile.TemporaryDirectory() as folder:
                        result = c.download_attachment(self.metadata(b'abc'), folder)
                        self.assertEqual(result['status'], 'failed')
                        self.assertEqual(result['error_code'], 'tls')
                        self.assertEqual(result['attempts'], 1)
                    self.assertEqual(c.opener.open.call_count, 1)

    def test_tls_interruptions_stop_after_three_attempts(self):
        c = self.client()
        c.opener.open.side_effect = jira.ssl.SSLEOFError('stream ended')
        with tempfile.TemporaryDirectory() as folder:
            result = c.download_attachment(self.metadata(b'abc'), folder)
            self.assertEqual(result['status'], 'failed')
            self.assertEqual(result['error_code'], 'network')
            self.assertEqual(result['attempts'], 3)
        self.assertEqual(c.opener.open.call_count, 3)
        self.assertEqual([call.args[0] for call in self.sleep.call_args_list], [1, 2])

    def test_reparse_attribute_rejected_without_is_junction(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder).resolve()
            child = root / 'TEST-1'
            child.mkdir()
            original_lstat = Path.lstat

            def lstat(path):
                if path == child:
                    return SimpleNamespace(st_mode=0o040755, st_file_attributes=0x400)
                return original_lstat(path)

            with patch.object(Path, 'is_junction', Mock(side_effect=AssertionError('New Python API must not be used')), create=True), patch.object(Path, 'lstat', lstat):
                with self.assertRaisesRegex(jira.JiraError, 'directory links'):
                    jira.contained_path(root, child)

    def test_internal_directory_links_are_rejected(self):
        for purpose in ('issue', 'attachment'):
            with self.subTest(purpose=purpose), tempfile.TemporaryDirectory() as folder:
                root = Path(folder).resolve()
                destination = root / 'other-issue'
                destination.mkdir()
                link = root / ('TEST-1' if purpose == 'issue' else 'attachments')
                if os.name == 'nt':
                    created = subprocess.run(['cmd', '/c', 'mklink', '/J', str(link), str(destination)], capture_output=True)
                    self.assertEqual(created.returncode, 0, created.stderr)
                else:
                    link.symlink_to(destination, target_is_directory=True)
                try:
                    c = self.client()
                    if purpose == 'issue':
                        c.issue = Mock(return_value={'key': 'TEST-1', 'fields': {'attachment': []}})
                        c.comments = Mock(return_value={'total': 0, 'comments': []})
                        with self.assertRaises(jira.JiraError):
                            c.archive_issue('TEST-1', root)
                    else:
                        result = c.download_attachment(self.metadata(b'abc'), link, archive_root=root)
                        self.assertEqual(result['status'], 'failed')
                        c.opener.open.assert_not_called()
                    self.assertEqual(list(destination.iterdir()), [])
                finally:
                    link.rmdir() if os.name == 'nt' else link.unlink()

    def test_publication_refuses_existing_file(self):
        with tempfile.TemporaryDirectory() as folder:
            source, target = Path(folder) / 'new.partial', Path(folder) / 'old.bin'
            source.write_bytes(b'new')
            target.write_bytes(b'old')
            with self.assertRaises(OSError):
                jira.publish_new_file(source, target)
            self.assertEqual(target.read_bytes(), b'old')
            self.assertEqual(source.read_bytes(), b'new')

    def test_publication_failure_keeps_partial_not_formal_file(self):
        c = self.client()
        c.opener.open.return_value = Response(b'abc')
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(jira, 'publish_new_file', side_effect=OSError('unavailable')):
                result = c.download_attachment(self.metadata(b'abc'), folder)
            self.assertEqual(result['status'], 'failed')
            self.assertEqual(result['error_code'], 'destination')
            self.assertEqual(len(list(Path(folder).iterdir())), 1)
            self.assertTrue(result['path'].endswith('.partial'))

    def test_archive_directory_link_escape(self):
        c = self.client()
        c.issue = Mock(return_value={'key': 'TEST-1', 'fields': {'attachment': []}})
        c.comments = Mock(return_value={'total': 0, 'comments': []})
        with tempfile.TemporaryDirectory() as folder:
            root, outside = Path(folder) / 'archive', Path(folder) / 'outside'
            root.mkdir()
            outside.mkdir()
            link = root / 'TEST-1'
            if os.name == 'nt':
                created = subprocess.run(['cmd', '/c', 'mklink', '/J', str(link), str(outside)], capture_output=True)
                self.assertEqual(created.returncode, 0, created.stderr)
            else:
                link.symlink_to(outside, target_is_directory=True)
            try:
                with self.assertRaises(jira.JiraError):
                    c.archive_issue('TEST-1', root)
                self.assertEqual(list(outside.iterdir()), [])
            finally:
                link.rmdir() if os.name == 'nt' else link.unlink()

    def test_attachment_directory_link_escape(self):
        c = self.client()
        with tempfile.TemporaryDirectory() as folder:
            root, outside = Path(folder) / 'archive', Path(folder) / 'outside'
            root.mkdir()
            outside.mkdir()
            link = root / 'attachments'
            if os.name == 'nt':
                created = subprocess.run(['cmd', '/c', 'mklink', '/J', str(link), str(outside)], capture_output=True)
                self.assertEqual(created.returncode, 0, created.stderr)
            else:
                link.symlink_to(outside, target_is_directory=True)
            try:
                result = c.download_attachment(self.metadata(b'abc'), link, archive_root=root)
                self.assertEqual(result['status'], 'failed')
                c.opener.open.assert_not_called()
                self.assertEqual(list(outside.iterdir()), [])
            finally:
                link.rmdir() if os.name == 'nt' else link.unlink()

    def test_login_to_archive_integrated(self):
        c = self.client()
        body = bytes(range(256))
        responses = [
            Response(b'form'), Response(b'ok'),
            Response(json.dumps({'name': 'synthetic'}).encode()),
            Response(json.dumps({'key': 'TEST-1', 'fields': {'summary': '中文', 'attachment': [self.metadata(body)]}}).encode()),
            Response(json.dumps({'total': 2, 'startAt': 0, 'comments': [{'id': '1'}]}).encode()),
            Response(json.dumps({'total': 2, 'startAt': 1, 'comments': [{'id': '2'}]}).encode()),
            Response(body),
        ]
        c.opener.open.side_effect = responses
        values = dict.fromkeys(('username', 'password'), 'synthetic-input')
        c.login(**values)
        with tempfile.TemporaryDirectory() as folder:
            saved = c.archive_issue('TEST-1', folder)
            manifest = json.loads((saved / 'attachments-manifest.json').read_text(encoding='utf-8'))
            self.assertEqual(manifest[0]['status'], 'downloaded')
            self.assertEqual((saved / manifest[0]['path']).read_bytes(), body)
            comments = json.loads((saved / 'comments.json').read_text(encoding='utf-8'))
            self.assertEqual(len(comments['comments']), 2)
        self.assertEqual(c.opener.open.call_count, 7)

    def test_archive_snapshot_and_json(self):
        c = self.client()
        c.issue = Mock(return_value={'key': 'TEST-1', 'fields': {'summary': '中文', 'attachment': []}})
        c.comments = Mock(return_value={'total': 1, 'comments': [{'id': '1', 'body': 'not verified'}]})
        with tempfile.TemporaryDirectory() as folder:
            first = c.archive_issue('TEST-1', folder)
            second = c.archive_issue('TEST-1', folder)
            self.assertEqual(second.parent, first)
            self.assertTrue(second.name.startswith('snapshot-'))
            for name in ('README.md', 'issue.json', 'comments.json', 'attachments-manifest.json'):
                self.assertTrue((first / name).is_file())
            self.assertEqual(json.loads((first / 'issue.json').read_text(encoding='utf-8'))['fields']['summary'], '中文')


if __name__ == '__main__':
    unittest.main(verbosity=2)
