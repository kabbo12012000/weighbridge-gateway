<?php

namespace App\Http\Controllers;

use App\Http\Requests\StoreWeighmentRequest;
use App\Models\Weighment;
use Carbon\CarbonImmutable;
use Illuminate\Database\UniqueConstraintViolationException;
use Illuminate\Http\JsonResponse;

class WeighmentController extends Controller
{
    /**
     * Saves a weighment. Only called by the operator's explicit Save action; the gateway
     * never writes here. Idempotent per Idempotency-Key; one reading can be saved once.
     */
    public function store(StoreWeighmentRequest $request): JsonResponse
    {
        $key = strtolower($request->header('Idempotency-Key'));

        if ($existing = Weighment::where('idempotency_key', $key)->first()) {
            return response()->json(['id' => $existing->id, 'duplicate' => true]);
        }

        $r = $request->validated('reading');
        try {
            $weighment = Weighment::create([
                'weighbridge_device_id' => $request->device->id,
                'device_id' => $r['device_id'],
                'vehicle_number' => strtoupper(trim($request->validated('vehicle_number'))),
                'customer_id' => $request->validated('customer_id'),
                'product_id' => $request->validated('product_id'),
                'weight' => $r['weight'],
                'unit' => $r['unit'],
                'stable' => (bool) $r['stable'],
                'stability_source' => $r['stability_source'] ?? null,
                'weight_type' => $r['weight_type'] ?? null,
                'raw_data' => $r['raw_data'] ?? null,
                'reading_id' => $r['reading_id'],
                'signature' => strtolower($r['signature']),
                'measured_at' => CarbonImmutable::parse($r['measured_at']),
                'captured_at' => CarbonImmutable::parse($r['captured_at']),
                'idempotency_key' => $key,
                'created_by' => $request->user()->id,
            ]);
        } catch (UniqueConstraintViolationException) {
            // Lost a race with the same request, or this reading was already used.
            if ($existing = Weighment::where('idempotency_key', $key)->first()) {
                return response()->json(['id' => $existing->id, 'duplicate' => true]);
            }

            return response()->json(['message' => 'This weight reading has already been saved. Capture a new reading.'], 409);
        }

        return response()->json(['id' => $weighment->id], 201);
    }
}
