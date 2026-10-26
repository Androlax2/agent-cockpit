import io
import base64
import json
from pathlib import Path
import ssl
import subprocess
import sys
import tarfile
import tempfile
import unittest


INSPECTOR = Path(__file__).with_name('verify_agent_docker_build.py')


def archive_bytes(files):
    contents = io.BytesIO()
    with tarfile.open(fileobj=contents, mode='w') as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))
    return contents.getvalue()


class ImageTrustTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='image-trust-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.certificate = ssl.create_default_context().get_ca_certs(binary_form=True)[0]
        self.pem = ssl.DER_cert_to_PEM_cert(self.certificate).encode()
        self.ca_file = self.root / 'proxy-ca.pem'
        self.ca_file.write_bytes(self.pem)
        self.archive = self.root / 'image.tar'

    def inspect(self, layers, config=None):
        files = {'config.json': json.dumps(config or {'config': {'Env': ['PATH=/usr/bin']}}).encode()}
        names = []
        for number, layer in enumerate(layers):
            name = f'layer-{number}/layer.tar'
            names.append(name)
            files[name] = archive_bytes(layer)
        files['manifest.json'] = json.dumps([{'Config': 'config.json', 'Layers': names, 'RepoTags': ['test:latest']}]).encode()
        self.archive.write_bytes(archive_bytes(files))
        return subprocess.run([sys.executable, str(INSPECTOR), str(self.archive), str(self.ca_file)],
                              capture_output=True, text=True)

    def test_clean_image_allows_build_history_but_no_runtime_overrides(self):
        config = {'config': {'Env': ['PATH=/usr/bin']},
                  'history': [{'created_by': 'RUN export CURL_CA_BUNDLE=/run/agent-build-trust/ca.pem; curl https://example.com'}]}
        result = self.inspect([{'usr/bin/app': b'clean'}], config)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('1 layers', result.stdout)

    def test_ca_in_an_earlier_deleted_layer_is_rejected(self):
        result = self.inspect([{'tmp/ca.pem': self.pem}, {'tmp/.wh.ca.pem': b''}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('tmp/ca.pem', result.stderr)
        self.assertIn('certificate', result.stderr)

    def test_der_and_rewrapped_pem_certificates_are_detected(self):
        body = b''.join(self.pem.splitlines()[1:-1])
        wrapped = b'-----BEGIN CERTIFICATE-----\r\n' + b'\r\n'.join(body[index:index + 32] for index in range(0, len(body), 32)) + b'\r\n-----END CERTIFICATE-----\r\n'
        for certificate in [self.certificate, wrapped]:
            with self.subTest(format=certificate[:25]):
                result = self.inspect([{'etc/ssl/custom': certificate}])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('certificate', result.stderr)

    def test_persisted_runtime_environment_is_rejected(self):
        result = self.inspect([{}], {'config': {'Env': ['CURL_CA_BUNDLE=/run/agent-build-trust/ca.pem']}})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('runtime configuration', result.stderr)

    def test_persisted_client_configuration_is_rejected(self):
        result = self.inspect([{'etc/apt/apt.conf.d/agent': b'Acquire::https::CaInfo "/run/agent-build-trust/ca.pem";'}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('etc/apt/apt.conf.d/agent', result.stderr)

    def test_a_certificate_embedded_in_runtime_config_is_rejected(self):
        result = self.inspect([{}], {'config': {'Env': ['CA_DATA=' + base64.b64encode(self.certificate).decode()]}})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('runtime configuration', result.stderr)

    def test_certificate_scanning_crosses_read_boundaries(self):
        result = self.inspect([{'etc/large-bundle': b'x' * (1024 * 1024 - 30) + self.pem}])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('certificate', result.stderr)

    def test_layer_paths_cannot_escape_the_archive(self):
        self.archive.write_bytes(archive_bytes({'manifest.json': b'[{"Config":"/etc/passwd","Layers":[]} ]'}))
        result = subprocess.run([sys.executable, str(INSPECTOR), str(self.archive), str(self.ca_file)],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('archive', result.stderr)


if __name__ == '__main__':
    unittest.main()
