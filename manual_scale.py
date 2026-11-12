import serial

# Change this to the port paired with the gateway's serial port.
PORT = "COM3"
BAUD = 9600  # Match the baud rate in the Tauri Serial port tab.

ser = serial.Serial(PORT, baudrate=BAUD, timeout=1)
print(f"Connected to {PORT}. Type a weight in kg (e.g., 25450) or 'q' to quit.")

try:
    while True:
        val = input("Enter weight (kg): ").strip()
        if val.lower() == "q":
            break
        if not val.isdigit():
            print("Enter a valid positive integer.")
            continue

        # Format matches the parser: ST (stable), GS (gross), 6-digit weight.
        frame = f"ST,GS,+{int(val):06d} kg\r\n"
        ser.write(frame.encode("ascii"))
        print(f"Sent -> {frame.strip()}")
finally:
    ser.close()
