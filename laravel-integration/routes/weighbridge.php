<?php

// Include from routes/web.php (session auth + CSRF, same as the rest of the React/Inertia app).

use App\Http\Controllers\WeighmentController;
use App\Models\WeighbridgeDevice;
use Illuminate\Support\Facades\Route;

Route::middleware(['auth'])->prefix('weighbridge')->name('weighbridge.')->group(function () {
    Route::post('weighments', [WeighmentController::class, 'store'])
        ->middleware('throttle:30,1')
        ->name('weighments.store');

    // What the weighment page needs to reach the local gateway. Gate this to operators.
    // (Or pass the same array as an Inertia prop when rendering the weighment page.)
    Route::get('devices/{device:device_id}/gateway', function (WeighbridgeDevice $device) {
        abort_unless($device->is_active, 404);

        return [
            'device_id' => $device->device_id,
            'gateway_token' => $device->gateway_token,
            'require_stable' => $device->require_stable,
        ];
    })->name('devices.gateway');
});
