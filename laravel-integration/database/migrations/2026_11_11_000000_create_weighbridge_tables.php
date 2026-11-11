<?php

use Illuminate\Database\Migrations\Migration;
use Illuminate\Database\Schema\Blueprint;
use Illuminate\Support\Facades\Schema;

return new class extends Migration
{
    public function up(): void
    {
        Schema::create('weighbridge_devices', function (Blueprint $table) {
            $table->id();
            $table->string('device_id', 64)->unique();          // must equal the gateway's Device ID
            $table->string('name');
            $table->text('gateway_token');                        // encrypted cast; given to operators' browsers
            $table->text('signing_secret');                       // encrypted cast; NEVER sent to the browser
            $table->decimal('max_weight', 12, 3)->default(200000);
            $table->decimal('min_weight', 12, 3)->default(0);
            $table->boolean('require_stable')->default(true);
            $table->unsignedInteger('max_reading_age_seconds')->default(1800);
            $table->boolean('is_active')->default(true);
            $table->timestamps();
        });

        Schema::create('weighments', function (Blueprint $table) {
            $table->id();
            $table->foreignId('weighbridge_device_id')->constrained('weighbridge_devices');
            $table->string('device_id', 64);
            $table->string('vehicle_number', 32)->index();
            $table->unsignedBigInteger('customer_id')->index();   // add FK to your customers table
            $table->unsignedBigInteger('product_id')->index();    // add FK to your products table
            $table->decimal('weight', 12, 3);
            $table->string('unit', 8);
            $table->boolean('stable');
            $table->string('stability_source', 16)->nullable();
            $table->string('weight_type', 16)->nullable();
            $table->string('raw_data', 1024)->nullable();
            $table->string('reading_id', 64)->unique();           // one gateway reading -> at most one weighment
            $table->char('signature', 64);
            $table->timestamp('measured_at', 3);                  // gateway time of the reading
            $table->timestamp('captured_at', 3);                  // when the operator pressed Capture
            $table->uuid('idempotency_key')->unique();            // safe retries / double clicks
            $table->foreignId('created_by')->nullable()->constrained('users');
            $table->timestamps();
        });
    }

    public function down(): void
    {
        Schema::dropIfExists('weighments');
        Schema::dropIfExists('weighbridge_devices');
    }
};
