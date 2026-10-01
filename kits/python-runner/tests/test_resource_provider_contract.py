from __future__ import annotations

import pytest

from mutsuki_runner_kit.contracts.codec import to_json_dict, to_json_value
from mutsuki_runner_kit.contracts.resource_provider import (
    ResourceDescriptorInvalidation,
    ResourceProviderRequest,
    ResourceProviderResponse,
)


def test_provider_failure_keeps_committed_invalidation() -> None:
    raw = {
        "result": {
            "Err": {
                "code": "resource.not_found",
                "source": "provider",
                "route": "partial.batch",
                "evidence": {},
                "cause": None,
                "lost_capability": None,
                "recovery": None,
            }
        },
        "invalidations": [{"provider_id": "provider", "ref_id": "removed", "generation": 1}],
    }
    response = ResourceProviderResponse.from_json_dict(raw)
    assert response.invalidations == (ResourceDescriptorInvalidation("provider", "removed", 1),)
    assert to_json_dict(response) == raw


def test_provider_request_preserves_externally_tagged_wire_shape() -> None:
    raw = {"CreateBlob": {"schema": "bytes.v1", "bytes": [1, 2, 255]}}
    assert to_json_value(ResourceProviderRequest.from_json_dict(raw)) == raw
    with pytest.raises(TypeError):
        ResourceProviderRequest.from_json_dict(
            {"CreateBlob": {"schema": "bytes.v1", "bytes": [256]}}
        )
    with pytest.raises(TypeError):
        ResourceProviderRequest.from_json_dict({"Unknown": {}})


@pytest.mark.parametrize("generation", [-1, 2**64, True])
def test_invalidation_requires_u64_generation(generation: int) -> None:
    with pytest.raises(TypeError):
        ResourceDescriptorInvalidation.from_json_dict(
            {"provider_id": "provider", "ref_id": "removed", "generation": generation}
        )
