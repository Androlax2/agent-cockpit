import json
import os
from pathlib import Path
import signal
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
RUNNER = REPO / 'config/bin/.local/bin/agent-docker-build'

# Docker is the external boundary: inspect files while the real CLI owns them.
DOCKER = '''import json, os, pathlib, sys
arguments = sys.argv[1:]
record = {'arguments': arguments, 'environment': {key: os.environ.get(key) for key in
          ('DOCKER_CONTEXT', 'BUILDX_BUILDER', 'BUILDKIT_HOST', 'DOCKER_HOST')}}
if 'config' in arguments:
    print(os.environ['COMPOSE_MODEL'])
else:
    if 'compose' in arguments:
        filename = pathlib.Path(arguments[arguments.index('-f') + 1])
        model = json.loads(filename.read_text())
        record['model'] = model
        service = next(iter(model['services'].values()))
        dockerfile = pathlib.Path(service['build']['dockerfile'])
        secrets = {key: pathlib.Path(value['file']).read_text() for key, value in model['secrets'].items()
                   if 'file' in value}
        if '--print' in arguments:
            print(json.dumps({'group': {'default': {'targets': ['web']}}, 'target': {'web': {
                'context': service['build']['context'], 'dockerfile': str(dockerfile),
                'secret': [{'id': key, 'src': value['file']} for key, value in model['secrets'].items() if 'file' in value]}}}))
    elif 'bake' in arguments:
        filename = pathlib.Path(arguments[arguments.index('-f') + 1])
        definition = json.loads(filename.read_text())
        target = definition['target']['web']
        dockerfile = pathlib.Path(target['dockerfile'])
        secrets = {secret['id']: pathlib.Path(secret['src']).read_text() for secret in target['secret']}
    else:
        filename = pathlib.Path(arguments[arguments.index('-f') + 1])
        dockerfile = filename
        secrets = {}
        for index, argument in enumerate(arguments):
            if argument == '--secret':
                fields = dict(part.split('=', 1) for part in arguments[index + 1].split(','))
                secrets[fields['id']] = pathlib.Path(fields['src']).read_text()
    record.update(filename=str(filename), dockerfile=str(dockerfile), text=dockerfile.read_text(), secrets=secrets)
    ignore = pathlib.Path(str(dockerfile) + '.dockerignore')
    record['ignore'] = ignore.read_text() if ignore.exists() else None
with open(os.environ['DOCKER_LOG'], 'a') as output:
    output.write(json.dumps(record) + '\\n')
if os.environ.get('WAIT_FOR_SIGNAL'):
    import time
    print('BUILD_READY', flush=True)
    time.sleep(30)
sys.exit(0 if 'config' in arguments or '--print' in arguments else int(os.environ.get('BUILD_STATUS', '0')))
'''


class DockerBuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='docker-trust-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / 'home'
        self.project = self.home / 'IdeaProjects/example'
        self.project.mkdir(parents=True)
        subprocess.run(['git', 'init', '-q', str(self.project)], check=True)
        self.context = self.project / 'build context'
        self.context.mkdir()
        self.dockerfile = self.context / 'Dockerfile'
        self.original = 'FROM ubuntu:24.04\nRUN echo first && \\\n    echo second\n'
        self.dockerfile.write_text(self.original)
        self.socket_path = self.project / '.agent-docker.sock'
        self.daemon = socket.socket(socket.AF_UNIX)
        self.daemon.bind(str(self.socket_path))
        self.addCleanup(self.daemon.close)
        self.bundle = self.root / 'bundle.pem'
        certificates = ssl.create_default_context().get_ca_certs(binary_form=True)
        self.bundle.write_text(''.join(ssl.DER_cert_to_PEM_cert(cert) for cert in certificates))
        self.config = self.root / 'docker-config'
        self.config.mkdir()
        (self.config / 'config.json').write_text('{"proxies":{"default":{"httpsProxy":"http://proxy:43051"}}}')
        self.tools = self.root / 'bin'
        self.tools.mkdir()
        docker = self.tools / 'docker'
        docker.write_text(f'#!{sys.executable}\n' + DOCKER)
        docker.chmod(0o755)
        self.log = self.root / 'docker-log'
        self.environment = os.environ | {
            'HOME': str(self.home), 'PATH': f'{self.tools}:{os.environ["PATH"]}',
            'DOCKER_HOST': f'unix://{self.socket_path}', 'DOCKER_CONFIG': str(self.config),
            'AGENT_DOCKER_CA_BUNDLE': str(self.bundle), 'DOCKER_LOG': str(self.log),
            'DOCKER_CONTEXT': 'other', 'BUILDX_BUILDER': 'remote', 'BUILDKIT_HOST': 'tcp://remote:1234',
        }

    def run_helper(self, *arguments, environment=None):
        return subprocess.run([sys.executable, str(RUNNER), *arguments], cwd=self.project,
                              env=self.environment | (environment or {}), capture_output=True, text=True)

    def records(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def build(self, *arguments, **options):
        return self.run_helper('build', *arguments, str(self.context), **options)

    def test_build_mounts_trust_without_persisting_it(self):
        result = self.build('-t', 'test/image', '--build-arg', 'VALUE=has spaces')
        self.assertEqual(result.returncode, 0, result.stderr)
        record = self.records()[-1]
        self.assertEqual(record['arguments'][:2], ['--host', f'unix://{self.socket_path}'])
        self.assertIn('default', record['arguments'])
        self.assertIn('VALUE=has spaces', record['arguments'])
        self.assertIn('test/image', record['arguments'])
        self.assertIn('type=secret', record['text'])
        self.assertIn('required=true', record['text'])
        self.assertIn('CURL_CA_BUNDLE=', record['text'])
        self.assertIn('APT_CONFIG=', record['text'])
        self.assertIn('PHP_INI_SCAN_DIR=', record['text'])
        self.assertIn('NODE_EXTRA_CA_CERTS=', record['text'])
        self.assertNotIn('\nENV ', record['text'])
        self.assertNotIn('\nCOPY ', record['text'])
        self.assertIn(self.bundle.read_text(), record['secrets'].values())
        self.assertEqual(self.dockerfile.read_text(), self.original)
        self.assertFalse(Path(record['filename']).exists())
        self.assertNotIn(str(self.context), record['filename'])
        self.assertTrue(all(record['environment'][key] is None for key in
                            ('DOCKER_CONTEXT', 'BUILDX_BUILDER', 'BUILDKIT_HOST')))

    def test_multistage_exec_and_existing_mounts_keep_their_commands(self):
        self.dockerfile.write_text('FROM ubuntu AS first\nRUN --mount=type=cache,target=/cache echo one\n'
                                   'FROM first\nRUN ["printf", "%s", "a $literal"]\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.records()[-1]['text']
        self.assertIn('--mount=type=cache,target=/cache', text)
        command = json.loads(next(line[line.index('['):] for line in text.splitlines() if line.startswith('RUN ') and '[' in line))
        self.assertEqual(command[-3:], ['printf', '%s', 'a $literal'])
        self.assertEqual(text.count('CURL_CA_BUNDLE='), 2)

    def test_non_root_build_processes_can_read_public_trust_mounts(self):
        self.dockerfile.write_text('FROM ubuntu\nUSER 1000\nRUN test -r "$CURL_CA_BUNDLE"\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        mounts = re.findall(r'--mount=([^\s]+)', self.records()[-1]['text'])
        self.assertEqual(len(mounts), 3)
        for mount in mounts:
            fields = dict(part.split('=', 1) for part in mount.split(','))
            mode = int(fields.get('mode', '0400'), 8)
            self.assertTrue(mode & 0o004, f'non-root apt/PHP cannot read {mount}')

    def test_node_fetch_uses_the_existing_proxy_only_during_build_steps(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.records()[-1]['text']
        self.assertIn('NODE_USE_ENV_PROXY=1', text)
        self.assertNotIn('\nENV ', text)

    def test_shell_run_keeps_quoting_pipeline_and_continuation_comments(self):
        self.dockerfile.write_text('FROM ubuntu\nRUN printf "%s" "a b" | cat && \\\n# comment\n    echo "$HOME"\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.records()[-1]['text']
        self.assertIn('printf "%s" "a b" | cat &&', text)
        self.assertIn('echo "$HOME"', text)
        self.assertNotIn('# comment', text)

    def test_dockerfile_specific_ignore_survives_relocation(self):
        Path(str(self.dockerfile) + '.dockerignore').write_text('private/**\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.records()[-1]['ignore'], 'private/**\n')

    def test_failed_build_returns_status_and_cleans_generated_files(self):
        result = self.build(environment={'BUILD_STATUS': '37'})
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertFalse(Path(self.records()[-1]['filename']).exists())
        self.assertEqual(self.dockerfile.read_text(), self.original)

    def test_sigterm_cleans_the_generated_inputs(self):
        process = subprocess.Popen([sys.executable, str(RUNNER), 'build', str(self.context)],
                                   cwd=self.project, env=self.environment | {'WAIT_FOR_SIGNAL': '1'},
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            self.assertEqual(process.stdout.readline().strip(), 'BUILD_READY')
            generated = Path(self.records()[-1]['filename'])
            self.assertTrue(generated.exists())
            process.send_signal(signal.SIGTERM)
            _, error = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 130, error)
            self.assertFalse(generated.exists())
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()

    def test_an_escaped_trailing_backslash_is_not_a_continuation(self):
        self.dockerfile.write_text('FROM ubuntu\nRUN printf hello\\\\\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.records()[-1]['text'].endswith('printf hello\\\\\n'))

    def test_three_trailing_backslashes_do_not_merge_run_instructions(self):
        self.dockerfile.write_text('FROM ubuntu\nRUN printf hello' + '\\' * 3 + '\nRUN echo second\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = self.records()[-1]['text'].splitlines()
        self.assertEqual(sum(line.startswith('RUN ') for line in lines), 2)
        self.assertTrue(lines[-1].endswith('echo second'))

    def test_leading_shell_bracket_test_is_not_exec_json(self):
        self.dockerfile.write_text('FROM ubuntu\nRUN [ -d /tmp ] && echo ok\n')
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        command = self.records()[-1]['text'].split('; ', 1)[1]
        executed = subprocess.run(['/bin/sh', '-c', command], capture_output=True, text=True)
        self.assertEqual(executed.returncode, 0, executed.stderr)
        self.assertEqual(executed.stdout, 'ok\n')

    def test_php_preserves_inherited_scan_paths_in_shell_and_exec_run(self):
        for inherited, expected in [(None, ':/run/agent-build-trust/php'),
                                    ('', '/run/agent-build-trust/php'),
                                    ('/custom/php:/extensions', '/custom/php:/extensions:/run/agent-build-trust/php')]:
            for instruction in ['RUN printf "%s" "$PHP_INI_SCAN_DIR"',
                                'RUN ["python3", "-c", "import os; print(os.environ[\\"PHP_INI_SCAN_DIR\\"], end=\\"\\")"]']:
                with self.subTest(inherited=inherited, instruction=instruction):
                    self.dockerfile.write_text('FROM ubuntu\n' + instruction + '\n')
                    result = self.build()
                    self.assertEqual(result.returncode, 0, result.stderr)
                    run = next(line for line in self.records()[-1]['text'].splitlines() if line.startswith('RUN '))
                    body = re.sub(r'^RUN (?:--mount=\S+ )+', '', run)
                    command = json.loads(body) if body.startswith('[') else ['/bin/sh', '-c', body]
                    environment = os.environ.copy()
                    environment.pop('PHP_INI_SCAN_DIR', None)
                    if inherited is not None:
                        environment['PHP_INI_SCAN_DIR'] = inherited
                    executed = subprocess.run(command, env=environment, capture_output=True, text=True)
                    self.assertEqual(executed.returncode, 0, executed.stderr)
                    self.assertEqual(executed.stdout, expected)

    def test_unsupported_syntax_fails_before_docker(self):
        for content in ['FROM ubuntu\nRUN <<EOF\necho hi\nEOF\n', 'FROM ubuntu\nSHELL ["fish", "-c"]\n',
                        '# escape=`\nFROM ubuntu\nRUN echo hi\n', 'FROM ubuntu\nONBUILD RUN echo hi\n',
                        '# syntax=custom/frontend\nFROM ubuntu\n', 'FROM ubuntu\nRUN echo /run/agent-build-trust/ca.pem\n']:
            with self.subTest(content=content):
                self.dockerfile.write_text(content)
                result = self.build()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Dockerfile:', result.stderr)
                self.assertEqual(self.records(), [])

    def test_wrong_daemon_missing_bundle_and_conflicting_flags_fail_early(self):
        for environment in [{'DOCKER_HOST': 'unix:///var/run/docker.sock'},
                            {'AGENT_DOCKER_CA_BUNDLE': str(self.root / 'missing')},
                            {'DOCKER_CONFIG': ''}]:
            with self.subTest(environment=environment):
                result = self.build(environment=environment)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.records(), [])
        result = self.build('--builder', 'remote')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.records(), [])

    def test_parallel_clone_uses_its_repository_relay(self):
        clone = self.home / 'IdeaProjects/.parallel/example/my-clone'
        clone.mkdir(parents=True)
        subprocess.run(['git', 'init', '-q', str(clone)], check=True)
        relay = clone.parent / '.agent-docker/docker.sock'
        relay.parent.mkdir()
        daemon = socket.socket(socket.AF_UNIX)
        self.addCleanup(daemon.close)
        daemon.bind(str(relay))
        self.project = clone
        result = self.build(environment={'DOCKER_HOST': f'unix://{relay}'})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.records()[-1]['arguments'][:2], ['--host', f'unix://{relay}'])

    def compose_model(self):
        return {'name': 'example', 'services': {
            'web': {'build': {'context': str(self.context), 'dockerfile': 'Dockerfile',
                             'args': {'WWWGROUP': '1000', 'LITERAL': '$HOME'},
                             'secrets': [{'source': 'token', 'target': 'token'}]},
                    'image': 'sail-8.4/app', 'environment': {'PRIVATE_RUNTIME': 'do-not-serialize'}},
            'other': {'build': {'context': '/unrelated'}}},
            'secrets': {'token': {'environment': 'BUILD_TOKEN'}, 'unused': {'environment': 'RUNTIME_TOKEN'}}}

    def test_compose_preserves_selected_build_and_omits_runtime_credentials(self):
        result = self.run_helper('compose', '-f', 'custom.yaml', 'web',
                                 environment={'COMPOSE_MODEL': json.dumps(self.compose_model())})
        self.assertEqual(result.returncode, 0, result.stderr)
        records = self.records()
        self.assertIn('custom.yaml', records[0]['arguments'])
        record = next(record for record in records if 'model' in record)
        model = record['model']
        self.assertEqual(list(model['services']), ['web'])
        self.assertEqual(model['services']['web']['image'], 'sail-8.4/app')
        self.assertEqual(model['services']['web']['build']['context'], str(self.context))
        self.assertEqual(model['services']['web']['build']['args'], {'WWWGROUP': '1000', 'LITERAL': '$$HOME'})
        self.assertEqual(model['secrets']['token'], {'environment': 'BUILD_TOKEN'})
        self.assertNotIn('unused', model['secrets'])
        self.assertNotIn('PRIVATE_RUNTIME', json.dumps(model))
        self.assertNotIn('do-not-serialize', json.dumps(model))
        self.assertFalse(Path(record['filename']).exists())
        self.assertEqual(self.dockerfile.read_text(), self.original)

    def test_compose_grants_only_its_generated_directory_to_bake(self):
        result = self.run_helper('compose', 'web', environment={'COMPOSE_MODEL': json.dumps(self.compose_model())})
        self.assertEqual(result.returncode, 0, result.stderr)
        record = self.records()[-1]
        self.assertIn('bake', record['arguments'])
        grants = [argument for argument in record['arguments'] if argument.startswith('--allow=')]
        self.assertEqual(grants, [f'--allow=fs.read={Path(record["filename"]).parent}'])
        self.assertNotIn('BUILDX_BAKE_ENTITLEMENTS_FS=0', str(record))

    def test_compose_refuses_unsupported_contexts_before_building(self):
        for extra in [{'context': 'https://example.com/repo.git'}, {'dockerfile_inline': 'FROM ubuntu'},
                      {'additional_contexts': {'base': 'service:other'}}, {'privileged': True}]:
            with self.subTest(extra=extra):
                if self.log.exists():
                    self.log.unlink()
                model = self.compose_model()
                model['services']['web']['build'].update(extra)
                result = self.run_helper('compose', 'web', environment={'COMPOSE_MODEL': json.dumps(model)})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.records()), 1)
                self.assertIn('config', self.records()[0]['arguments'])


if __name__ == '__main__':
    unittest.main()
