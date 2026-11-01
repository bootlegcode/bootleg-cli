"""Disposable kind deployment; every kubectl invocation uses a private kubeconfig.

Mirrors smuggler/examples/kind.yaml (two replicas, app + worker per pod).
Never selects or deletes an existing cluster or namespace.
"""
import ipaddress
import json
import os
import subprocess

NODE_IMAGE = "kindest/node:v1.32.11@sha256:5fc52d52a7b9574015299724bd68f183702956aa4a2116ae75a63cb574b35af8"


class KindAcceptance:
    def __init__(self, name, home, network):
        self.name = name
        self.kubeconfig = home / "kind-kubeconfig"
        self.env = dict(os.environ, KIND_EXPERIMENTAL_DOCKER_NETWORK=network)
        self.failure_category = None

    def run(self, *args, **kwargs):
        try:
            return subprocess.check_output(args, text=True, env=self.env, timeout=300,
                                           stderr=subprocess.PIPE, **kwargs).strip()
        except subprocess.TimeoutExpired:
            self.failure_category = "command_timeout"
            raise
        except subprocess.CalledProcessError as error:
            # Classify known diagnostics without persisting raw command output,
            # kubeconfig paths, secret manifests or Kubernetes log contents.
            text = (error.stderr or "").lower()
            self.failure_category = "command_failed"
            for phrase, category in (("no space left on device", "disk_exhausted"),
                                     ("failed to pull image", "image_pull_failed"),
                                     ("timed out", "readiness_timeout"),
                                     ("deadline exceeded", "readiness_timeout")):
                if phrase in text:
                    self.failure_category = category
                    break
            raise

    def kubectl(self, *args, **kwargs):
        return self.run("kubectl", "--kubeconfig", str(self.kubeconfig),
                        "--context", "kind-" + self.name, *args, **kwargs)

    def start(self, image, origin, project_id, token_file, fail_after_secret=False):
        self.run("kind", "create", "cluster", "--name", self.name, "--image", NODE_IMAGE,
                 "--kubeconfig", str(self.kubeconfig), "--wait", "180s")
        self.kubeconfig.chmod(0o600)
        self.run("kind", "load", "docker-image", "--name", self.name, image)
        # Docker Desktop resolves this name at the node boundary, not necessarily in CoreDNS.
        aliases = []
        if "host.docker.internal" in origin:
            address = self.run("docker", "exec", self.name + "-control-plane", "getent",
                               "ahostsv4", "host.docker.internal").split()[0]
            ipaddress.IPv4Address(address)
            aliases = [dict(ip=address, hostnames=["host.docker.internal"])]
        self.kubectl("create", "namespace", "acceptance")
        self.kubectl("-n", "acceptance", "create", "secret", "generic", "bootleg-project",
                     "--from-file=token=" + str(token_file))
        if fail_after_secret:
            self.failure_category = "injected_after_secret"
            raise RuntimeError("injected partial kind provisioning failure")
        containers = []
        for name in ("app", "worker"):
            fields = {"POD_NAME": "metadata.name", "POD_UID": "metadata.uid", "POD_NAMESPACE": "metadata.namespace"}
            environment = [dict(name=key, valueFrom={"fieldRef": {"fieldPath": value}})
                           for key, value in fields.items()]
            environment += [dict(name=key, value=value) for key, value in {
                "BOOTLEG_API_URL": origin,
                "BOOTLEG_PROJECT_ID": project_id, "BOOTLEG_ALLOW_INSECURE": "true",
                "BOOTLEG_CONTAINER_NAME": name, "BOOTLEG_CLUSTER_NAME": self.name,
            }.items()]
            environment.append(dict(name="BOOTLEG_PROJECT_TOKEN", valueFrom={
                "secretKeyRef": {"name": "bootleg-project", "key": "token"}}))
            containers.append(dict(name=name, image=image, imagePullPolicy="Never", env=environment))
        deployment = dict(apiVersion="apps/v1", kind="Deployment",
                          metadata=dict(name="smuggler", namespace="acceptance"), spec=dict(
                              replicas=2, selector=dict(matchLabels=dict(app="smuggler")),
                              template=dict(metadata=dict(labels=dict(app="smuggler")), spec=dict(
                                  automountServiceAccountToken=False, terminationGracePeriodSeconds=15,
                                  hostAliases=aliases,
                                  containers=containers))))
        self.kubectl("create", "-f", "-", input=json.dumps(deployment))
        self.kubectl("-n", "acceptance", "rollout", "status", "deployment/smuggler", "--timeout=180s")
        pods = json.loads(self.kubectl("-n", "acceptance", "get", "pods", "-l", "app=smuggler", "-o", "json"))["items"]
        assert len(pods) == 2
        return {pod["metadata"]["uid"]: pod["metadata"]["name"] for pod in pods}

    def cleanup(self):
        # The name was generated by this run; never infer it from current-context.
        result = subprocess.run(["kind", "delete", "cluster", "--name", self.name,
                                 "--kubeconfig", str(self.kubeconfig)], env=self.env,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)
        return result.returncode == 0
