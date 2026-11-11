<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;

class WeighbridgeDevice extends Model
{
    protected $fillable = [
        'device_id', 'name', 'gateway_token', 'signing_secret', 'max_weight', 'min_weight',
        'require_stable', 'max_reading_age_seconds', 'is_active',
    ];

    // Never serialise the signing secret to the frontend. The gateway token is exposed
    // deliberately and only through WeighbridgeGatewayController::show (authorised users).
    protected $hidden = ['gateway_token', 'signing_secret'];

    protected function casts(): array
    {
        return [
            'gateway_token' => 'encrypted',
            'signing_secret' => 'encrypted',
            'max_weight' => 'decimal:3',
            'min_weight' => 'decimal:3',
            'require_stable' => 'boolean',
            'is_active' => 'boolean',
        ];
    }
}
