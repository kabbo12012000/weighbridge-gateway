<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\BelongsTo;

class Weighment extends Model
{
    protected $fillable = [
        'weighbridge_device_id', 'device_id', 'vehicle_number', 'customer_id', 'product_id',
        'weight', 'unit', 'stable', 'stability_source', 'weight_type', 'raw_data',
        'reading_id', 'signature', 'measured_at', 'captured_at', 'idempotency_key', 'created_by',
    ];

    protected function casts(): array
    {
        return [
            'weight' => 'decimal:3',
            'stable' => 'boolean',
            'measured_at' => 'datetime',
            'captured_at' => 'datetime',
        ];
    }

    public function device(): BelongsTo
    {
        return $this->belongsTo(WeighbridgeDevice::class, 'weighbridge_device_id');
    }
}
