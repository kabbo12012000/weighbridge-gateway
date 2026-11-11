<?php

namespace App\Http\Requests;

use App\Models\WeighbridgeDevice;
use App\Support\Weighbridge\ReadingSignature;
use Carbon\CarbonImmutable;
use Illuminate\Foundation\Http\FormRequest;
use Illuminate\Support\Facades\Log;
use Illuminate\Validation\Validator;

class StoreWeighmentRequest extends FormRequest
{
    public ?WeighbridgeDevice $device = null;

    public function authorize(): bool
    {
        // Replace with your permission check, e.g. $this->user()->can('create', Weighment::class)
        return $this->user() !== null;
    }

    public function rules(): array
    {
        return [
            'vehicle_number' => ['required', 'string', 'max:32'],
            'customer_id' => ['required', 'integer'], // add exists:customers,id
            'product_id' => ['required', 'integer'],  // add exists:products,id
            'reading' => ['required', 'array'],
            'reading.device_id' => ['required', 'string', 'max:64'],
            'reading.reading_id' => ['required', 'string', 'max:64'],
            'reading.weight' => ['required', 'numeric'],
            'reading.unit' => ['required', 'string', 'max:8'],
            'reading.stable' => ['required', 'boolean'],
            'reading.stability_source' => ['nullable', 'string', 'in:indicator,motion'],
            'reading.weight_type' => ['nullable', 'string', 'max:16'],
            'reading.raw_data' => ['nullable', 'string', 'max:1024'],
            // Kept as the exact string the gateway signed; parsed only after verification.
            'reading.measured_at' => ['required', 'string', 'max:40'],
            'reading.signature' => ['required', 'string', 'size:64'],
            'reading.captured_at' => ['required', 'date'],
        ];
    }

    public function after(): array
    {
        return [function (Validator $v) {
            if ($v->errors()->isNotEmpty()) {
                return;
            }
            $key = (string) $this->header('Idempotency-Key');
            if (! preg_match('/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i', $key)) {
                $v->errors()->add('reading', 'Missing or invalid Idempotency-Key header.');

                return;
            }

            $r = $this->input('reading');
            $device = WeighbridgeDevice::where('device_id', $r['device_id'])->where('is_active', true)->first();
            if (! $device) {
                $v->errors()->add('reading.device_id', 'Unknown or inactive weighbridge.');

                return;
            }
            if (! ReadingSignature::valid($r, $device->signing_secret)) {
                Log::warning('Weighbridge reading signature mismatch', [
                    'device_id' => $r['device_id'], 'reading_id' => $r['reading_id'], 'user_id' => $this->user()?->id,
                ]);
                $v->errors()->add('reading', 'The weight reading could not be verified. Capture the weight again.');

                return;
            }

            $weight = (float) $r['weight'];
            if ($weight < (float) $device->min_weight || $weight > (float) $device->max_weight) {
                $v->errors()->add('reading.weight', "Weight must be between {$device->min_weight} and {$device->max_weight}.");
            }
            if ($device->require_stable && ! filter_var($r['stable'], FILTER_VALIDATE_BOOLEAN)) {
                $v->errors()->add('reading.stable', 'Only a stable weight can be saved.');
            }

            try {
                $measured = CarbonImmutable::parse($r['measured_at']);
            } catch (\Throwable) {
                $v->errors()->add('reading.measured_at', 'Invalid reading time.');

                return;
            }
            // The gateway PC's clock is used for measured_at; allow a little skew.
            $age = now()->diffInSeconds($measured, false) * -1;
            if ($age > $device->max_reading_age_seconds || $age < -120) {
                $v->errors()->add('reading.measured_at', 'The captured weight is too old (or the weighbridge PC clock is wrong). Capture it again.');
            }

            $this->device = $device;
        }];
    }
}
